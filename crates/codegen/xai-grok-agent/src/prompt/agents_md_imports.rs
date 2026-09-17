//! `@path` imports inside project-instruction files.
//!
//! A CLAUDE.md that says `@AGENTS.md` is asking for that file's content, the
//! way Claude Code reads it. Before this module the line was delivered as
//! literal text and the referenced file was never read, so a repo that keeps
//! its rules in one file and points every vendor's entry point at it shipped
//! the pointer and none of the rules.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How many hops an import chain may take. A imports B imports C is depth 3.
/// Claude Code stops at 5 and so does this.
pub const MAX_IMPORT_DEPTH: usize = 5;

/// A resolved import: the file an `@ref` named, and the text it holds.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImportedFile {
    pub path: PathBuf,
    pub content: String,
}

/// Trailing punctuation that belongs to the sentence, not to the path.
/// `.` is absent on purpose: it is how every file extension starts.
const TRAILING_PUNCTUATION: &[char] = &[',', ';', ':', '!', '?', ')', ']', '}', '"', '\'', '>'];

/// Whether a line opens or closes a fenced code block.
fn is_code_fence(line: &str) -> bool {
    let trimmed = line.trim_start();
    trimmed.starts_with("```") || trimmed.starts_with("~~~")
}

/// Drop the inline code spans from one line. Backtick-delimited runs are
/// copied out as spaces so the byte offsets of the surrounding text, and the
/// whitespace boundary an `@` needs, both survive.
fn strip_inline_code(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut in_code = false;
    for ch in line.chars() {
        if ch == '`' {
            in_code = !in_code;
            out.push(' ');
        } else if in_code {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

/// Read the `@ref` tokens out of one line of instruction text.
fn imports_in_line(line: &str) -> Vec<String> {
    let line = strip_inline_code(line);
    let mut refs = Vec::new();
    let bytes = line.as_bytes();
    let mut index = 0;
    while let Some(offset) = line[index..].find('@') {
        let at = index + offset;
        index = at + 1;
        // An `@` that follows a word character is an email address or a Rust
        // pattern binding, never an import.
        if at > 0 {
            let previous = bytes[at - 1] as char;
            if !previous.is_whitespace() && previous != '(' && previous != '[' {
                continue;
            }
        }
        let rest = &line[at + 1..];
        let end = rest.find(char::is_whitespace).unwrap_or(rest.len());
        let token = rest[..end].trim_end_matches(TRAILING_PUNCTUATION);
        if !token.is_empty() {
            refs.push(token.to_string());
        }
        index = at + 1 + end;
    }
    refs
}

/// Read every `@ref` out of an instruction file's text, in order, skipping
/// fenced code blocks and inline code spans.
pub fn parse_imports(content: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut in_fence = false;
    for line in content.lines() {
        if is_code_fence(line) {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        refs.extend(imports_in_line(line));
    }
    refs
}

/// Turn one `@ref` into the path it names. Relative refs resolve against the
/// directory of the file that wrote them, so a moved file takes its imports
/// with it.
fn resolve_import(
    reference: &str,
    importer_dir: &Path,
    home_dir: Option<&Path>,
) -> Option<PathBuf> {
    let path = if reference == "~" {
        home_dir?.to_path_buf()
    } else if let Some(rest) = reference.strip_prefix("~/") {
        home_dir?.join(rest)
    } else {
        let path = Path::new(reference);
        if path.is_absolute() {
            path.to_path_buf()
        } else {
            importer_dir.join(path)
        }
    };
    path.is_file().then_some(path)
}

/// Canonicalize for cycle detection, falling back to the path as written.
fn canonical(path: &Path) -> PathBuf {
    dunce::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Collect what `content` imports, depth-first, in the order the refs appear.
///
/// `seen` carries every path already delivered by this discovery pass —
/// including the files discovery found on its own — so an import of a file
/// that is already in the prompt adds nothing and a cycle terminates.
///
/// A gitignored file IS imported. The ref is a deliberate instruction to read
/// it, which is what makes an ignored local-override file importable at all.
pub fn collect_imports(
    importer: &Path,
    content: &str,
    home_dir: Option<&Path>,
    seen: &mut HashSet<PathBuf>,
    depth: usize,
) -> Vec<ImportedFile> {
    if depth >= MAX_IMPORT_DEPTH {
        return Vec::new();
    }
    let Some(importer_dir) = importer.parent() else {
        return Vec::new();
    };

    let mut imported = Vec::new();
    for reference in parse_imports(content) {
        let Some(path) = resolve_import(&reference, importer_dir, home_dir) else {
            continue;
        };
        let key = canonical(&path);
        if !seen.insert(key) {
            continue;
        }
        let Ok(child_content) = std::fs::read_to_string(&path) else {
            continue;
        };
        let nested = collect_imports(&path, &child_content, home_dir, seen, depth + 1);
        imported.push(ImportedFile {
            path,
            content: child_content,
        });
        imported.extend(nested);
    }
    imported
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn parses_a_bare_reference() {
        assert_eq!(parse_imports("@AGENTS.md"), vec!["AGENTS.md"]);
        assert_eq!(
            parse_imports("see @docs/rules.md now"),
            vec!["docs/rules.md"]
        );
    }

    #[test]
    fn parses_every_reference_on_a_line() {
        assert_eq!(parse_imports("@a.md @b.md"), vec!["a.md", "b.md"]);
    }

    #[test]
    fn ignores_an_email_address_and_a_decorator() {
        assert!(parse_imports("mail matthaynie4@gmail.com today").is_empty());
        assert!(parse_imports("value@1.0.0").is_empty());
    }

    #[test]
    fn ignores_a_reference_inside_code() {
        assert!(parse_imports("write `@AGENTS.md` to import it").is_empty());
        assert!(parse_imports("```\n@AGENTS.md\n```").is_empty());
        assert!(parse_imports("~~~\n@AGENTS.md\n~~~").is_empty());
    }

    #[test]
    fn reads_a_reference_after_a_closed_fence() {
        assert_eq!(
            parse_imports("```\n@inside.md\n```\n@outside.md"),
            vec!["outside.md"]
        );
    }

    #[test]
    fn strips_sentence_punctuation_but_keeps_the_extension() {
        assert_eq!(parse_imports("read @a.md, then stop"), vec!["a.md"]);
        assert_eq!(parse_imports("read (@a.md)"), vec!["a.md"]);
        assert_eq!(parse_imports("read @docs/a.md."), vec!["docs/a.md."]);
    }

    #[test]
    fn resolves_relative_to_the_importing_file() {
        let tmp = tempfile::tempdir().unwrap();
        let docs = tmp.path().join("docs");
        fs::create_dir_all(&docs).unwrap();
        fs::write(docs.join("child.md"), "child").unwrap();
        let importer = tmp.path().join("CLAUDE.md");
        fs::write(&importer, "@docs/child.md").unwrap();

        let mut seen = HashSet::from([canonical(&importer)]);
        let imported = collect_imports(&importer, "@docs/child.md", None, &mut seen, 1);
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].content, "child");
    }

    #[test]
    fn resolves_a_home_reference() {
        let tmp = tempfile::tempdir().unwrap();
        let home = tmp.path().join("home");
        fs::create_dir_all(&home).unwrap();
        fs::write(home.join("global.md"), "global").unwrap();
        let importer = tmp.path().join("CLAUDE.md");

        let mut seen = HashSet::new();
        let imported = collect_imports(&importer, "@~/global.md", Some(&home), &mut seen, 1);
        assert_eq!(imported.len(), 1);
        assert_eq!(imported[0].content, "global");
    }

    #[test]
    fn skips_a_reference_that_names_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let importer = tmp.path().join("CLAUDE.md");
        fs::create_dir_all(tmp.path().join("adir")).unwrap();

        let mut seen = HashSet::new();
        let imported = collect_imports(&importer, "@missing.md @adir", None, &mut seen, 1);
        assert!(imported.is_empty());
    }

    #[test]
    fn follows_a_chain_and_stops_at_the_depth_limit() {
        let tmp = tempfile::tempdir().unwrap();
        // 1.md -> 2.md -> ... -> 7.md, all written up front.
        for index in 1..=7 {
            fs::write(
                tmp.path().join(format!("{index}.md")),
                format!("body-{index}\n@{}.md", index + 1),
            )
            .unwrap();
        }
        let importer = tmp.path().join("1.md");
        let content = fs::read_to_string(&importer).unwrap();

        let mut seen = HashSet::from([canonical(&importer)]);
        let imported = collect_imports(&importer, &content, None, &mut seen, 1);
        let bodies: Vec<&str> = imported
            .iter()
            .map(|file| file.content.lines().next().unwrap())
            .collect();
        // Depth 1 is the importer itself, so four more hops are taken.
        assert_eq!(bodies, vec!["body-2", "body-3", "body-4", "body-5"]);
    }

    #[test]
    fn a_cycle_terminates_and_delivers_each_file_once() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("a.md"), "a-body\n@b.md").unwrap();
        fs::write(tmp.path().join("b.md"), "b-body\n@a.md").unwrap();
        let importer = tmp.path().join("a.md");
        let content = fs::read_to_string(&importer).unwrap();

        let mut seen = HashSet::from([canonical(&importer)]);
        let imported = collect_imports(&importer, &content, None, &mut seen, 1);
        assert_eq!(imported.len(), 1);
        assert!(imported[0].content.starts_with("b-body"));
    }

    #[test]
    fn a_file_already_in_the_prompt_is_not_imported_again() {
        let tmp = tempfile::tempdir().unwrap();
        let sibling = tmp.path().join("AGENTS.md");
        fs::write(&sibling, "shared").unwrap();
        let importer = tmp.path().join("CLAUDE.md");

        let mut seen = HashSet::from([canonical(&importer), canonical(&sibling)]);
        let imported = collect_imports(&importer, "@AGENTS.md", None, &mut seen, 1);
        assert!(imported.is_empty());
    }

    #[test]
    fn nested_imports_follow_their_own_file() {
        let tmp = tempfile::tempdir().unwrap();
        fs::write(tmp.path().join("child.md"), "child\n@grandchild.md").unwrap();
        fs::write(tmp.path().join("grandchild.md"), "grandchild").unwrap();
        fs::write(tmp.path().join("sibling.md"), "sibling").unwrap();
        let importer = tmp.path().join("CLAUDE.md");

        let mut seen = HashSet::new();
        let imported = collect_imports(&importer, "@child.md @sibling.md", None, &mut seen, 1);
        let bodies: Vec<&str> = imported
            .iter()
            .map(|file| file.content.lines().next().unwrap())
            .collect();
        assert_eq!(bodies, vec!["child", "grandchild", "sibling"]);
    }
}
