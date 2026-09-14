//! Helper that mines the "next concrete step" inlined into the goal
//! continuation nudge from the planner-emitted plan file.
//!
//! Verifier-verdict gaps are delivered on a separate, more prominent
//! path (`render_verifier_gaps_block` in `acp_session.rs`, fed from the
//! persisted `last_classifier_gaps` summary), so this module only reads
//! the plan's next unchecked item. If that fails, the caller falls back
//! to a generic "check your todo list" line, so the helper here never
//! returns it itself.
//!
//! The reader caps the file at [`MAX_READ_BYTES`] (8 KiB) and never
//! panics — parse failures yield `None`. When the cap is reached the
//! reader drops the trailing potentially-incomplete line so we never
//! surface a half-truncated bullet to the nudge.
//!
//! The output is inlined as plain text in the nudge body, never as a
//! file pointer.

use std::path::Path;

/// 8 KiB cap on per-file reads. Each goal nudge fires per turn, so a
/// hostile or runaway verdict/plan must not blow up the context.
pub(crate) const MAX_READ_BYTES: usize = 8 * 1024;

/// Read up to [`MAX_READ_BYTES`] from `path`. Returns `None` on any
/// I/O failure (missing file, permission denied, etc.). When the
/// buffer reaches the cap, the trailing potentially-incomplete line
/// is dropped so a bullet spanning the cap boundary cannot leak a
/// half-truncated tail upstream.
fn read_capped(path: &Path) -> Option<String> {
    use std::io::Read;
    let file = std::fs::File::open(path).ok()?;
    let mut buf = Vec::with_capacity(MAX_READ_BYTES.min(4096));
    file.take(MAX_READ_BYTES as u64)
        .read_to_end(&mut buf)
        .ok()?;
    if buf.len() >= MAX_READ_BYTES
        && let Some(last_nl) = buf.iter().rposition(|b| *b == b'\n')
    {
        buf.truncate(last_nl);
    }
    Some(String::from_utf8_lossy(&buf).into_owned())
}

/// First unchecked `- [ ]` (or `* [ ]` / `+ [ ]`) markdown checkbox in a
/// plan file; `- [x]` / `- [X]` are skipped, `None` when none remain.
///
/// Numbered `## Acceptance criteria` are deliberately NOT mined: they are
/// the judged contract (WHAT must hold), never get checked off, so
/// surfacing criterion 1 as the "next step" repeated a stale line forever.
///
/// When the plan has a `## Task checklist` section only its checkboxes
/// are mined; otherwise the whole file is scanned except `## Non-goals`
/// and `## Deviations` (out-of-scope by definition).
pub(crate) fn first_unchecked_plan_item(path: &Path) -> Option<String> {
    let body = read_capped(path)?;
    extract_first_unchecked(&body)
}

/// Case-insensitive match of a markdown header line (`#`-prefixed at any
/// level) against a section `name` ("task checklist", "non-goals", ...).
fn is_section_header(line: &str, name: &str) -> bool {
    let trimmed = line.trim_start();
    if !trimmed.starts_with('#') {
        return false;
    }
    let title = trimmed.trim_start_matches('#').trim();
    title.eq_ignore_ascii_case(name)
}

fn is_any_header(line: &str) -> bool {
    line.trim_start().starts_with('#')
}

/// Markdown heading level (number of leading `#`), 0 for non-headers.
fn header_level(line: &str) -> usize {
    line.trim_start().chars().take_while(|c| *c == '#').count()
}

/// Iterate checkboxes within the `## Task checklist` section only.
/// Deeper subheaders (e.g. `### Phase 1`) stay inside the section; only
/// a header at the checklist's own level or shallower ends it.
fn first_unchecked_in_checklist(body: &str) -> Option<String> {
    let mut section_level: Option<usize> = None;
    for line in body.lines() {
        if is_section_header(line, "task checklist") {
            section_level = Some(header_level(line));
            continue;
        }
        let Some(level) = section_level else {
            continue;
        };
        if is_any_header(line) && header_level(line) <= level {
            return None; // section ended; no unchecked item found
        }
        if let Some(item) = parse_checkbox_item(line.trim_start()) {
            return Some(item);
        }
    }
    None
}

/// Sections whose checkboxes must never be mined as a next step.
const EXCLUDED_SECTIONS: &[&str] = &["non-goals", "deviations"];

fn extract_first_unchecked(body: &str) -> Option<String> {
    let has_checklist = body.lines().any(|l| is_section_header(l, "task checklist"));
    if has_checklist {
        return first_unchecked_in_checklist(body);
    }
    let mut excluded = false;
    for line in body.lines() {
        if is_any_header(line) {
            excluded = EXCLUDED_SECTIONS
                .iter()
                .any(|name| is_section_header(line, name));
            continue;
        }
        if excluded {
            continue;
        }
        if let Some(item) = parse_checkbox_item(line.trim_start()) {
            return Some(item);
        }
    }
    None
}

/// Strip the leading `- ` / `* ` / `+ ` bullet marker. `None` when
/// the line does not begin with a recognised bullet glyph.
fn strip_bullet_marker(trimmed: &str) -> Option<&str> {
    trimmed
        .strip_prefix("- ")
        .or_else(|| trimmed.strip_prefix("* "))
        .or_else(|| trimmed.strip_prefix("+ "))
        .map(str::trim_start)
}

/// Parse a `- [ ] foo` / `* [ ] foo` / `+ [ ] foo` markdown checkbox
/// for the plan extractor. STRICTLY requires the literal `[ ]` glyph
/// after the bullet marker — a plain `- foo` bullet returns `None`.
/// `- [x]` / `- [X]` (resolved) also return `None` so iteration
/// continues to the next unchecked item. Returns the bullet text
/// trimmed.
fn parse_checkbox_item(trimmed: &str) -> Option<String> {
    let after_marker = strip_bullet_marker(trimmed)?;
    let after_checkbox = after_marker.strip_prefix("[ ]")?;
    let text = after_checkbox.trim();
    (!text.is_empty()).then(|| text.to_string())
}

/// Every todo item a plan body names, in plan order.
///
/// This is the seeding source for the goal harness: the planner's plan is
/// published, and the steps it names become the session's todo list before
/// the goal-start reminder is rendered, so the model never has to re-read the
/// plan to transcribe it (and cannot lose a step doing so).
///
/// A `## Task checklist` section wins: every UNCHECKED `- [ ]` box inside it,
/// in order. `- [x]` / `- [X]` are skipped — a ticked box is work the plan
/// already calls done. When the plan has no checklist section at all
/// (`analysis` / `research` goals, which the planner prompt does not require a
/// checklist for), the `## Acceptance criteria` section is used instead: each
/// numbered (`1.`) or bulleted entry in order, with its list marker stripped.
///
/// A body that names neither yields NO items. Auto-seeding must never invent
/// work the plan did not state, so an empty or malformed body is empty here
/// rather than an error.
///
/// A wrapped item (markdown soft-wraps long steps) keeps its continuation
/// lines: they are joined onto the item, never dropped, so a step is never
/// silently truncated into its first line.
pub(crate) fn plan_todo_items(body: &str) -> Vec<String> {
    let checklist = items_in_section(body, "task checklist", SectionBody::CheckboxOnly);
    if !checklist.is_empty() || has_section(body, "task checklist") {
        return checklist;
    }
    items_in_section(body, "acceptance criteria", SectionBody::CheckboxOrList)
}

/// Every item a plan file names, capped at [`MAX_READ_BYTES`] like the
/// next-step reader. `None`-equivalent (an empty `Vec`) on any I/O failure.
pub(crate) fn plan_todo_items_from_path(path: &Path) -> Vec<String> {
    read_capped(path)
        .map(|body| plan_todo_items(&body))
        .unwrap_or_default()
}

/// What a section's list items look like.
#[derive(Clone, Copy)]
enum SectionBody {
    /// Only `- [ ]` checkboxes count (the `## Task checklist` shape).
    CheckboxOnly,
    /// Checkboxes and plain numbered/bulleted entries (`## Acceptance
    /// criteria`, which the planner writes as a numbered list).
    CheckboxOrList,
}

fn has_section(body: &str, name: &str) -> bool {
    body.lines().any(|l| is_section_header(l, name))
}

/// Collect the list items of one named section, in order.
///
/// Mirrors [`first_unchecked_in_checklist`]'s section scoping: a header at the
/// section's own level or shallower ends it; deeper subheaders stay inside it.
/// Blank lines and non-item prose are skipped, and a non-item line directly
/// after an item is treated as that item's wrapped continuation. A checkbox
/// line is never a continuation: a resolved `- [x]` box is skipped outright
/// rather than folded into the item above it.
fn items_in_section(body: &str, name: &str, shape: SectionBody) -> Vec<String> {
    let mut items: Vec<String> = Vec::new();
    let mut section_level: Option<usize> = None;
    for line in body.lines() {
        if is_section_header(line, name) {
            section_level = Some(header_level(line));
            continue;
        }
        let Some(level) = section_level else {
            continue;
        };
        if is_any_header(line) && header_level(line) <= level {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Some(text) = parse_checkbox_item(line.trim_start()) {
            items.push(text);
            continue;
        }
        if is_any_checkbox(line.trim_start()) {
            continue; // resolved (`- [x]`) or empty-labelled box
        }
        match (shape, parse_list_item(trimmed)) {
            (SectionBody::CheckboxOrList, Some(text)) => items.push(text.to_owned()),
            // Not a list item: a wrapped continuation of the previous one.
            _ => {
                if let Some(last) = items.last_mut() {
                    last.push(' ');
                    last.push_str(trimmed);
                }
            }
        }
    }
    items
}

/// True for any markdown checkbox line (`- [ ] x`, `- [x] x`, `- [X] x`,
/// `- [ ]`), whether or not it carries a label. Distinguishes "a box we are
/// skipping" from prose that belongs to the item above.
fn is_any_checkbox(trimmed: &str) -> bool {
    let Some(after_marker) = strip_bullet_marker(trimmed) else {
        return false;
    };
    after_marker.starts_with("[ ]")
        || after_marker.starts_with("[x]")
        || after_marker.starts_with("[X]")
}

/// Strip a numbered (`1.` / `1)`) or bulleted (`-` / `*` / `+`) list marker,
/// returning the entry text. `None` when the line is not a list item.
fn parse_list_item(trimmed: &str) -> Option<&str> {
    if let Some(rest) = strip_bullet_marker(trimmed) {
        let text = rest.trim();
        return (!text.is_empty()).then_some(text);
    }
    let digits = trimmed.chars().take_while(char::is_ascii_digit).count();
    if digits == 0 {
        return None;
    }
    let after = &trimmed[digits..];
    let rest = after
        .strip_prefix(". ")
        .or_else(|| after.strip_prefix(") "))
        .or_else(|| (after == "." || after == ")").then_some(""))?;
    let text = rest.trim();
    (!text.is_empty()).then_some(text)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn write_temp(body: &str) -> NamedTempFile {
        let f = NamedTempFile::new().expect("temp file must be creatable");
        std::fs::write(f.path(), body).expect("write must succeed");
        f
    }

    #[test]
    fn plan_extracts_first_unchecked_item() {
        let body = "# Plan\n\n- [x] done\n- [x] also done\n- [ ] write the integration test\n- [ ] ship it\n";
        assert_eq!(
            extract_first_unchecked(body).as_deref(),
            Some("write the integration test"),
        );
    }

    #[test]
    fn plan_tolerates_indented_and_alt_bullets() {
        let body = "  - [x] one\n   * [ ] indented star item\n";
        assert_eq!(
            extract_first_unchecked(body).as_deref(),
            Some("indented star item"),
        );
    }

    #[test]
    fn plan_returns_none_when_all_items_checked() {
        let body = "- [x] one\n- [x] two\n";
        assert!(extract_first_unchecked(body).is_none());
    }

    #[test]
    fn plan_returns_none_on_empty_unchecked_label() {
        let body = "- [ ]   \n- [ ] real item\n";
        assert_eq!(extract_first_unchecked(body).as_deref(), Some("real item"),);
    }

    /// Numbered `## Acceptance criteria` are the judged contract, NOT a
    /// task list, and must NOT be mined as a next step (they never get
    /// checked off, so criterion 1 would surface forever). Only genuine
    /// `- [ ]` checkboxes count; a plan of pure numbered criteria yields
    /// `None` so the caller falls back to the generic todo reminder.
    #[test]
    fn plan_does_not_mine_numbered_acceptance_criteria() {
        let numbered = "# Plan\n\n## Acceptance criteria\n\n1. app is created\n2. physics works\n";
        assert!(extract_first_unchecked(numbered).is_none());
        // Even with an inline `[ ]` glyph, a numbered item lacks the
        // bullet marker `parse_checkbox_item` requires, so it is ignored.
        let numbered_checkbox = "1. [ ] still a criterion, not a checkbox\n";
        assert!(extract_first_unchecked(numbered_checkbox).is_none());
    }

    /// `parse_checkbox_item` strictly requires the literal `[ ]`
    /// glyph, so a plan containing only plain `- foo` bullets (no
    /// checkbox) must yield `None` and let the directive nudge fall
    /// through to the generic todo reminder.
    #[test]
    fn plan_plain_bullet_without_checkbox_returns_none() {
        let body = "## Non-goals\n- a plain bullet\n- another plain bullet\n";
        assert!(extract_first_unchecked(body).is_none());
    }

    /// Pin the recognised checkbox variants: tight-form `- [ ]foo`
    /// (no space after `]`) is accepted; uppercase `- [X] done` is
    /// skipped as completed.
    #[test]
    fn plan_checkbox_edge_cases() {
        let no_space = "- [ ]foo no space\n";
        assert_eq!(
            extract_first_unchecked(no_space).as_deref(),
            Some("foo no space"),
        );

        let upper_x = "- [X] uppercase done\n- [ ] real\n";
        assert_eq!(extract_first_unchecked(upper_x).as_deref(), Some("real"),);
    }

    /// With a `## Task checklist` section present, only its checkboxes
    /// are mined; an unchecked box in another section is invisible.
    #[test]
    fn checklist_section_scopes_extraction() {
        let body = "# Plan\n\n## Task checklist\n- [x] scaffold\n- [ ] wire input handling\n\n## Notes\n- [ ] stray box elsewhere\n";
        assert_eq!(
            extract_first_unchecked(body).as_deref(),
            Some("wire input handling"),
        );
        // All checklist items done ⇒ None even though a stray box exists.
        let done = "## Task checklist\n- [x] scaffold\n\n## Notes\n- [ ] stray box\n";
        assert!(extract_first_unchecked(done).is_none());
    }

    /// Deeper subheaders inside the checklist do not end the section;
    /// a same-level header does.
    #[test]
    fn checklist_subheaders_do_not_end_the_section() {
        let body = "## Task checklist\n### Phase 1\n- [x] done\n### Phase 2\n- [ ] phase two step\n\n## Notes\n- [ ] stray box\n";
        assert_eq!(
            extract_first_unchecked(body).as_deref(),
            Some("phase two step"),
        );
        let ended = "## Task checklist\n- [x] done\n## Notes\n- [ ] stray box\n";
        assert!(extract_first_unchecked(ended).is_none());
    }

    /// Without a checklist section, checkboxes under `## Non-goals` and
    /// `## Deviations` must never be surfaced as the next step.
    #[test]
    fn non_goals_and_deviations_checkboxes_are_excluded() {
        let body =
            "## Non-goals\n- [ ] out-of-scope feature\n\n## Deviations\n- [ ] noted deviation\n";
        assert!(extract_first_unchecked(body).is_none());
        // A real checkbox after an excluded section is still found.
        let mixed = "## Non-goals\n- [ ] out of scope\n\n## Steps\n- [ ] real next step\n";
        assert_eq!(
            extract_first_unchecked(mixed).as_deref(),
            Some("real next step"),
        );
    }

    #[test]
    fn missing_file_returns_none() {
        let path = std::path::PathBuf::from("/definitely/not/a/real/path/xyzzy.md");
        assert!(first_unchecked_plan_item(&path).is_none());
    }

    #[test]
    fn malformed_markdown_returns_none_without_panicking() {
        let f = write_temp("# Plan\n\n");
        assert!(first_unchecked_plan_item(f.path()).is_none());
    }

    /// Invalid UTF-8 must not panic; `from_utf8_lossy` substitutes
    /// `U+FFFD` and the extractor returns the item after the garbage.
    #[test]
    fn invalid_utf8_plan_body_is_tolerated() {
        let f = NamedTempFile::new().unwrap();
        std::fs::write(f.path(), b"\xFF\xFE garbage\n- [ ] real step\n".as_slice()).unwrap();
        assert_eq!(
            first_unchecked_plan_item(f.path()).as_deref(),
            Some("real step"),
        );
    }

    /// The documented 8 KiB cap must not drift, and content past the
    /// cap must be invisible. Build a body strictly larger than the
    /// cap with a sentinel past the boundary, then assert both
    /// `read_capped` and the extractor hide the sentinel.
    #[test]
    fn read_is_capped_at_max_read_bytes() {
        assert_eq!(
            MAX_READ_BYTES,
            8 * 1024,
            "documented 8 KiB cap must not drift",
        );

        let mut body = String::new();
        body.push_str("- [ ] visible step\n");
        // Pad past the cap; lines after this point must stay invisible.
        while body.len() < MAX_READ_BYTES + 32 {
            body.push('x');
        }
        body.push_str("\n- [ ] HIDDEN past cap\n");
        assert!(body.len() > MAX_READ_BYTES + 16);

        let f = write_temp(&body);

        let raw = read_capped(f.path()).expect("read must succeed");
        assert!(
            !raw.contains("HIDDEN"),
            "read_capped must drop content past the cap: {raw}",
        );
        assert!(raw.contains("visible step"));

        let item = first_unchecked_plan_item(f.path()).expect("must extract within cap");
        assert!(
            !item.contains("HIDDEN"),
            "extractor must hide content past MAX_READ_BYTES: {item}",
        );
        assert!(item.contains("visible step"));
    }

    /// An item that spans the cap boundary must not be surfaced
    /// as a half-truncated tail.
    #[test]
    fn bullet_spanning_cap_boundary_is_dropped() {
        let mut body = String::new();
        body.push_str("- [ ] short visible step\n");
        // Pad just short of the cap, then plant an item that crosses it.
        while body.len() < MAX_READ_BYTES - 32 {
            body.push('x');
        }
        body.push_str("\n- [ ] ");
        let cross_text = "y".repeat(128);
        body.push_str(&cross_text);
        body.push('\n');
        assert!(body.len() > MAX_READ_BYTES);

        let f = write_temp(&body);
        let item = first_unchecked_plan_item(f.path()).expect("must extract within cap");
        assert_eq!(
            item, "short visible step",
            "must return the in-cap item; truncated tail must not leak: {item}",
        );
    }

    #[test]
    fn read_capped_handles_files_smaller_than_cap() {
        let f = write_temp("- [ ] tiny step\n");
        assert_eq!(
            first_unchecked_plan_item(f.path()).as_deref(),
            Some("tiny step"),
        );
    }

    // ---- plan_todo_items: the goal-harness seeding source ----

    /// The checklist is the seeding source, in plan order, one item per box.
    #[test]
    fn plan_todo_items_returns_every_checklist_box_in_order() {
        let body = "# Plan\n\n## Task checklist\n\
             - [ ] add the parser\n- [ ] wire it into publish\n- [ ] cover it with a test\n\
             \n## Non-goals\n- [ ] out of scope\n";
        assert_eq!(
            plan_todo_items(body),
            vec![
                "add the parser".to_string(),
                "wire it into publish".to_string(),
                "cover it with a test".to_string(),
            ],
        );
    }

    /// A ticked box is work the plan already calls done, so it is not seeded;
    /// it must also not be folded into the item above it.
    #[test]
    fn plan_todo_items_skips_resolved_boxes_without_gluing_them_on() {
        let body = "## Task checklist\n\
             - [x] scaffold\n- [X] uppercase done\n- [ ] the real next step\n";
        assert_eq!(plan_todo_items(body), vec!["the real next step".to_string()]);
    }

    #[test]
    fn plan_todo_items_tolerates_indented_and_alternate_bullets() {
        let body = "## Task checklist\n  - [ ] indented dash\n   * [ ] indented star\n+ [ ] plus\n";
        assert_eq!(
            plan_todo_items(body),
            vec![
                "indented dash".to_string(),
                "indented star".to_string(),
                "plus".to_string(),
            ],
        );
    }

    /// Sub-headers inside the checklist stay in it, and a same-level header
    /// ends it — a box under `## Notes` is not a plan step.
    #[test]
    fn plan_todo_items_scopes_to_the_checklist_section() {
        let body = "## Task checklist\n### Phase 1\n- [ ] phase one step\n\
             ## Notes\n- [ ] stray box\n";
        assert_eq!(plan_todo_items(body), vec!["phase one step".to_string()]);
    }

    /// With no checklist section at all (`analysis`/`research` plans), the
    /// numbered acceptance criteria are the seeding source, marker stripped.
    #[test]
    fn plan_todo_items_falls_back_to_numbered_acceptance_criteria() {
        let body = "# Plan\n\n## Acceptance criteria\n\
             1. the exporter writes a file\n\
             2. the loader reads it back\n\
             \n## Non-goals\n- [ ] not this\n";
        assert_eq!(
            plan_todo_items(body),
            vec![
                "the exporter writes a file".to_string(),
                "the loader reads it back".to_string(),
            ],
        );
    }

    /// The fallback also accepts bulleted criteria and `1)` numbering.
    #[test]
    fn plan_todo_items_fallback_accepts_bullets_and_paren_numbering() {
        let body = "## Acceptance criteria\n- a bulleted criterion\n1) a paren-numbered one\n";
        assert_eq!(
            plan_todo_items(body),
            vec![
                "a bulleted criterion".to_string(),
                "a paren-numbered one".to_string(),
            ],
        );
    }

    /// No checklist AND no criteria ⇒ nothing to seed. Auto-seeding never
    /// invents items the plan did not state.
    #[test]
    fn plan_todo_items_is_empty_when_the_plan_names_no_steps() {
        assert!(plan_todo_items("# Plan\n\n## Goal kind\nanalysis\n").is_empty());
        assert!(plan_todo_items("").is_empty());
        assert!(plan_todo_items("not markdown at all").is_empty());
        // A checklist of only resolved boxes seeds nothing, and does NOT fall
        // through to the criteria section (the plan did name a checklist).
        let all_done = "## Task checklist\n- [x] done\n\n## Acceptance criteria\n1. still here\n";
        assert!(plan_todo_items(all_done).is_empty());
    }

    /// A soft-wrapped step keeps its continuation: seeding must not truncate a
    /// step into its first line.
    #[test]
    fn plan_todo_items_joins_wrapped_continuations() {
        let body = "## Task checklist\n- [ ] a step that wraps\n  onto a second line\n- [ ] next\n";
        assert_eq!(
            plan_todo_items(body),
            vec![
                "a step that wraps onto a second line".to_string(),
                "next".to_string(),
            ],
        );
    }

    /// The documented 8 KiB cap binds seeding too: a box past the cap is
    /// invisible, exactly as it is for the next-step reader.
    #[test]
    fn plan_todo_items_respects_the_read_cap() {
        let mut body = String::from("## Task checklist\n- [ ] visible step\n");
        while body.len() < MAX_READ_BYTES + 32 {
            body.push('x');
        }
        body.push_str("\n- [ ] HIDDEN past cap\n");
        let f = write_temp(&body);

        let items = plan_todo_items_from_path(f.path());
        assert_eq!(items, vec!["visible step".to_string()]);
    }

    /// Invalid UTF-8 and a missing file are tolerated, never a panic.
    #[test]
    fn plan_todo_items_from_path_tolerates_bad_input() {
        assert!(plan_todo_items_from_path(&std::path::PathBuf::from(
            "/definitely/not/a/real/path/xyzzy.md"
        ))
        .is_empty());

        let f = NamedTempFile::new().unwrap();
        std::fs::write(
            f.path(),
            b"\xFF\xFE garbage\n## Task checklist\n- [ ] real step\n".as_slice(),
        )
        .unwrap();
        let items = plan_todo_items_from_path(f.path());
        assert_eq!(items, vec!["real step".to_string()]);
    }
}
