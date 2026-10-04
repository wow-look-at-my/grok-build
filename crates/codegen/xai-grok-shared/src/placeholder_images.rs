//! Shared helper for resolving `[Image #N: <path>]` placeholders into image bytes.

use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

/// Maximum size of a single placeholder-loaded image.
pub const MAX_PLACEHOLDER_IMAGE_BYTES: usize = 50_000_000;

pub const MAX_PLACEHOLDERS_PER_PROMPT: usize = 16;

/// Per-prompt aggregate-bytes cap across recovered placeholder images.
pub const MAX_PLACEHOLDER_AGGREGATE_BYTES: usize = 200 * 1024 * 1024;

/// `_meta` key under which an attached image's `[Image #N]` display number is recorded on its ACP image block.
pub const IMAGE_DISPLAY_NUMBER_META_KEY: &str = "xai.dev/imageDisplayNumber";

/// Build an ACP image-block `_meta` value carrying `display_number` under [`IMAGE_DISPLAY_NUMBER_META_KEY`].
pub fn display_number_meta(display_number: usize) -> agent_client_protocol::Meta {
    let mut meta = agent_client_protocol::Meta::new();
    meta.insert(
        IMAGE_DISPLAY_NUMBER_META_KEY.to_owned(),
        serde_json::json!(display_number),
    );
    meta
}

/// Read the `[Image #N]` display number recorded in an image block's `_meta`, if present.
pub fn display_number_from_meta(meta: Option<&agent_client_protocol::Meta>) -> Option<usize> {
    meta?
        .get(IMAGE_DISPLAY_NUMBER_META_KEY)?
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
}

/// Build the per-turn registry mapping `[Image #N]` numbers to references from the user's inline attached images.
/// See `xai_grok_tools::types::resources::AttachedImages`.
///
/// The display number comes from each block's `_meta` (set by the TUI), falling back to 1-based position for callers that don't record it.
/// The reference is one `image_edit`'s resolver can read directly.
/// It is the bare durable path (from the `file://` URI) when present, else a `data:<mime>;base64,<data>` URL.
pub fn attached_image_references(
    images: &[agent_client_protocol::ImageContent],
) -> Vec<(usize, String)> {
    images
        .iter()
        .enumerate()
        .map(|(idx, image)| {
            let display_number = display_number_from_meta(image.meta.as_ref()).unwrap_or(idx + 1);
            let reference =
                if let Some(path) = image.uri.as_deref().and_then(|u| u.strip_prefix("file://")) {
                    path.to_owned()
                } else {
                    format!("data:{};base64,{}", image.mime_type, image.data)
                };
            (display_number, reference)
        })
        .collect()
}

/// File extensions accepted by the placeholder loader.
pub const ALLOWED_IMAGE_EXTENSIONS: &[&str] =
    &["png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "tif"];

/// Substrings that, if present anywhere in a canonical path, deny the load
/// even when the parent prefix is in the allowlist.
pub const DENY_PATH_CONTAINS: &[&str] = &[
    ".photoslibrary/",
    ".musiclibrary/",
    ".imovielibrary/",
    "/.Trash/",
    "/Library/Keychains/",
    "/Library/Containers/",
    "/.ssh/",
    "/.aws/",
    "/.gnupg/",
];

/// Compiled regex matching the TUI placeholder format `[Image #<digits>:
/// <path>]`.
static IMAGE_PLACEHOLDER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\[Image #(\d+): ([^\]\r\n]+?)\]").expect("placeholder regex is valid")
});

/// One occurrence of `[Image #N: <path>]` in arbitrary text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlaceholderMatch {
    /// The `N` in `[Image #N: …]` (1-based per the TUI's convention).
    pub display_number: usize,
    /// The raw path string as it appeared in the text (no canonicalisation).
    pub path: String,
    /// Byte offsets into the source text covered by the full `[Image #N: …]` match; `text[start..end]` is the placeholder.
    pub span: (usize, usize),
}

/// Scan `text` for every well-formed placeholder. Malformed forms (`[Image #3]`, `[Image #5: ]`, truncated, etc.) are skipped without failing the whole scan. The regex iterator is short-circuited via [`Iterator::take`] **before** `filter_map`, so placeholders do not consume captures. At most [`MAX_PLACEHOLDERS_PER_PROMPT`] are inspected.
pub fn extract_placeholders(text: &str) -> Vec<PlaceholderMatch> {
    IMAGE_PLACEHOLDER_RE
        .captures_iter(text)
        .take(MAX_PLACEHOLDERS_PER_PROMPT)
        .filter_map(|cap| {
            let whole = cap.get(0)?;
            let n = cap.get(1)?.as_str().parse::<usize>().ok()?;
            let path = cap.get(2)?.as_str().trim();
            if path.is_empty() {
                return None;
            }
            Some(PlaceholderMatch {
                display_number: n,
                path: path.to_owned(),
                span: (whole.start(), whole.end()),
            })
        })
        .collect()
}

/// Rewrites every `[Image #N: <path>]` placeholder in `text` to the shorter
/// `[Image #N]` form, dropping the path component. Run **after** the
/// orphan-recovery pipeline has finished extracting paths it needs to load.
/// Once the image is attached inline the path is redundant *and harmful*. The
/// model treats it as a hint and may call the `Read` tool on the path even
/// though the bytes are already in context. The bracketed anchor `[Image #N]`
/// is preserved so the model can still tell where in the prose the image was
/// referenced. Takes `String` by value so the common no-placeholder case
/// returns the input unchanged with zero allocations.
pub fn strip_paths_from_image_placeholders(text: String) -> String {
    use std::fmt::Write as _;
    // Fast path: probe with `is_match` (no `Captures` allocation) and return the owned input unchanged when there is nothing to do
    if !IMAGE_PLACEHOLDER_RE.is_match(&text) {
        return text;
    }
    let mut out = String::with_capacity(text.len());
    let mut last = 0usize;
    for cap in IMAGE_PLACEHOLDER_RE
        .captures_iter(&text)
        .take(MAX_PLACEHOLDERS_PER_PROMPT)
    {
        let whole = cap.get(0).expect("regex match always has group 0");
        let n = cap.get(1).expect("regex always has group 1").as_str();
        if let Some(prefix) = text.get(last..whole.start()) {
            out.push_str(prefix);
        }
        // `write!` to a String is infallible.
        let _ = write!(out, "[Image #{n}]");
        last = whole.end();
    }
    if let Some(suffix) = text.get(last..) {
        out.push_str(suffix);
    }
    out
}

/// Loaded image bytes plus a sniffed MIME type.
#[derive(Debug, Clone)]
pub struct LoadedPlaceholderImage {
    /// Raw image bytes read from disk.
    pub data: Vec<u8>,
    /// MIME type derived from the `image` crate's full header parser.
    pub mime_type: String,
}

/// Why a placeholder load was rejected.
///
/// The variants are deliberately coarse-grained so an attacker with log access cannot probe the filesystem by reading distinct error messages.
/// No variant carries the raw `io::Error` string; an `io::ErrorKind` is retained where useful but renderer-side messages stay generic.
#[derive(Debug, thiserror::Error)]
pub enum PlaceholderLoadError {
    /// Resolved path is outside every entry in the prefix allowlist.
    #[error("path is outside allowed prefixes")]
    OutsideAllowedPrefixes,
    /// Path could not be canonicalised (missing, permission denied, etc.).
    #[error("path does not resolve")]
    CanonicalizeFailed,
    /// Extension is not in [`ALLOWED_IMAGE_EXTENSIONS`].
    #[error("unsupported image extension")]
    UnsupportedExtension,
    /// Resolved path is not a regular file (e.g. directory, FIFO).
    #[error("path is not a regular file")]
    NotAFile,
    /// `std::fs::read` failed after canonicalisation.
    #[error("read failed: {0:?}")]
    ReadFailed(std::io::ErrorKind),
    /// File exceeds the configured per-image byte cap.
    #[error("file is {actual} bytes, exceeds {limit}-byte cap")]
    TooLarge { actual: usize, limit: usize },
    /// Bytes do not decode as a supported image.
    #[error("bytes do not decode as a supported image")]
    NotAnImage,
}

/// Build the canonical prefix allowlist: the workspace cwd plus a small set
/// of common user-image directories under `$HOME`.
pub fn default_allowed_prefixes(workspace_cwd: &Path) -> Vec<PathBuf> {
    default_allowed_prefixes_with_home(workspace_cwd, xai_dirs::home_dir())
}

/// Variant of [`default_allowed_prefixes`] for tests: an explicit `home` avoids depending on the ambient `$HOME`.
/// The returned `Vec` is canonical and deduplicated; **ordering is alphabetical by OS path, not insertion order**.
/// The prefix check in [`load_placeholder_image`] uses `starts_with`, so order is functionally irrelevant.
pub fn default_allowed_prefixes_with_home(
    workspace_cwd: &Path,
    home: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut prefixes: Vec<PathBuf> = Vec::new();
    match dunce::canonicalize(workspace_cwd) {
        Ok(canon) => prefixes.push(canon),
        Err(e) => {
            tracing::warn!(
                workspace_cwd = ?workspace_cwd,
                error_kind = ?e.kind(),
                "placeholder_images: workspace cwd does not canonicalize; dropping from allowlist",
            );
        }
    }
    if let Some(home) = home {
        for sub in HOME_IMAGE_SUBDIRS {
            if let Ok(canon) = dunce::canonicalize(home.join(sub)) {
                prefixes.push(canon);
            }
        }
    }
    prefixes.sort();
    prefixes.dedup();
    prefixes
}

/// Subdirectories under `$HOME` that are part of the default allowlist. The
/// entries match the directories users paste images from.
pub const HOME_IMAGE_SUBDIRS: &[&str] = &[
    "Downloads",
    "Desktop",
    "Pictures",
    "Documents",
    "Screenshots",
];

/// Resolve and validate `path_str`, then read the file. Validation is
/// **prefix-first**: an out-of-allowlist path returns
/// [`PlaceholderLoadError::OutsideAllowedPrefixes`] before any other check
/// runs.
pub fn load_placeholder_image(
    path_str: &str,
    allowed_prefixes: &[PathBuf],
) -> Result<LoadedPlaceholderImage, PlaceholderLoadError> {
    load_placeholder_image_with_cap(path_str, allowed_prefixes, MAX_PLACEHOLDER_IMAGE_BYTES)
}

/// Variant of [`load_placeholder_image`] that takes an explicit byte cap.
pub fn load_placeholder_image_with_cap(
    path_str: &str,
    allowed_prefixes: &[PathBuf],
    max_bytes: usize,
) -> Result<LoadedPlaceholderImage, PlaceholderLoadError> {
    let canonical = dunce::canonicalize(Path::new(path_str))
        .map_err(|_| PlaceholderLoadError::CanonicalizeFailed)?;
    load_canonical_placeholder_image(&canonical, allowed_prefixes, max_bytes)
}

/// Variant of [`load_placeholder_image`] for callers that have already canonicalised the path.
/// [`recover_orphan_placeholders`] canonicalises once for dedup; this saves one `canonicalize` syscall per successful load.
pub fn load_canonical_placeholder_image(
    canonical: &Path,
    allowed_prefixes: &[PathBuf],
    max_bytes: usize,
) -> Result<LoadedPlaceholderImage, PlaceholderLoadError> {
    // Prefix check first: out-of-scope paths return a single, file-system-independent variant
    if !allowed_prefixes.iter().any(|p| canonical.starts_with(p)) {
        return Err(PlaceholderLoadError::OutsideAllowedPrefixes);
    }
    // Deny-list pass: even inside an allowed prefix, certain subtrees (macOS bundle internals, `.Trash`, secret stores).
    let canonical_str = canonical.to_string_lossy().replace('\\', "/");
    if DENY_PATH_CONTAINS
        .iter()
        .any(|needle| canonical_str.contains(needle))
    {
        return Err(PlaceholderLoadError::OutsideAllowedPrefixes);
    }

    let ext = canonical
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .ok_or(PlaceholderLoadError::UnsupportedExtension)?;
    if !ALLOWED_IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        return Err(PlaceholderLoadError::UnsupportedExtension);
    }

    let metadata = canonical
        .metadata()
        .map_err(|e| PlaceholderLoadError::ReadFailed(e.kind()))?;
    if !metadata.is_file() {
        return Err(PlaceholderLoadError::NotAFile);
    }
    let size = metadata.len() as usize;
    if size > max_bytes {
        return Err(PlaceholderLoadError::TooLarge {
            actual: size,
            limit: max_bytes,
        });
    }

    let data = std::fs::read(canonical).map_err(|e| PlaceholderLoadError::ReadFailed(e.kind()))?;
    // Re-check after read: a sparse/grown file may exceed the cap even when the metadata snapshot was under it
    if data.len() > max_bytes {
        return Err(PlaceholderLoadError::TooLarge {
            actual: data.len(),
            limit: max_bytes,
        });
    }

    // Image-decoder validation: a file with PNG magic bytes followed by arbitrary content (e.g. a private key).
    let mime_type = decode_image_mime(&data).ok_or(PlaceholderLoadError::NotAnImage)?;

    Ok(LoadedPlaceholderImage {
        data,
        mime_type: mime_type.to_owned(),
    })
}

/// Header-only validation via the shared image_validate helper.
/// Returns the matching MIME type, or `None` if the bytes fail validation.
fn decode_image_mime(data: &[u8]) -> Option<&'static str> {
    xai_grok_image::validate_image_bytes_with(data, false)
        .ok()
        .map(|(_, _, mime)| mime)
}

/// Recover orphan `[Image #N: <path>]` placeholders embedded in the user
/// query text by loading the referenced files from disk. Production wrapper
/// over [`recover_orphan_placeholders_with_prefixes`].
pub fn recover_orphan_placeholders(
    query: &str,
    raw_images: &mut Vec<agent_client_protocol::ImageContent>,
    workspace_cwd: &Path,
) -> usize {
    let allowed = default_allowed_prefixes(workspace_cwd);
    recover_orphan_placeholders_with_prefixes(query, raw_images, &allowed)
}

/// Variant of [`recover_orphan_placeholders`] that takes an explicit prefix
/// allowlist. An "orphan" is a placeholder whose canonical path is **not**
/// already present in `raw_images`. That is, the TUI did not send a matching
/// `ContentBlock::Image`. Dedup runs against the **canonical** form of each
/// existing `raw_images[i].uri`.
pub fn recover_orphan_placeholders_with_prefixes(
    query: &str,
    raw_images: &mut Vec<agent_client_protocol::ImageContent>,
    allowed_prefixes: &[PathBuf],
) -> usize {
    recover_orphan_placeholders_with_prefixes_and_caps(
        query,
        raw_images,
        allowed_prefixes,
        MAX_PLACEHOLDER_IMAGE_BYTES,
        MAX_PLACEHOLDER_AGGREGATE_BYTES,
    )
}

/// Variant of [`recover_orphan_placeholders_with_prefixes`] with injectable caps.
///
/// Production code calls the cap-defaulting wrapper.
///
/// The loop reads the next image, then checks `aggregate_bytes + image.len() > aggregate_max`.
/// The first image that pushes the running total **strictly above** the cap is dropped and the loop `break`s.
/// Earlier images already in `raw_images` stay.
/// The cap is therefore an **inclusive** upper bound: a total exactly equal to the cap is admitted, the next byte trips the break.
/// See test `recover_orphan_placeholders_aggregate_cap_inclusive_boundary`.
pub fn recover_orphan_placeholders_with_prefixes_and_caps(
    query: &str,
    raw_images: &mut Vec<agent_client_protocol::ImageContent>,
    allowed_prefixes: &[PathBuf],
    per_image_max: usize,
    aggregate_max: usize,
) -> usize {
    use base64::Engine as _;

    let placeholders = extract_placeholders(query);
    if placeholders.is_empty() {
        return 0;
    }

    // Pre-compute canonical paths of the already-attached images
    // Dedup then matches non-canonical `file://` URIs from the TUI as well as percent-encoded URIs from non-TUI clients
    let attached_canonical: std::collections::HashSet<PathBuf> = raw_images
        .iter()
        .filter_map(|img| img.uri.as_deref().and_then(canonical_from_file_uri))
        .collect();

    let mut recovered: usize = 0;
    let mut aggregate_bytes: usize = 0;
    for ph in placeholders {
        let canonical = match dunce::canonicalize(Path::new(&ph.path)) {
            Ok(c) => c,
            Err(_) => {
                tracing::warn!(
                    path = ?ph.path,
                    "placeholder_images: orphan path does not resolve; leaving placeholder text intact",
                );
                continue;
            }
        };
        if attached_canonical.contains(&canonical) {
            continue;
        }
        match load_canonical_placeholder_image(&canonical, allowed_prefixes, per_image_max) {
            Ok(loaded) => {
                let next_total = aggregate_bytes.saturating_add(loaded.data.len());
                if next_total > aggregate_max {
                    tracing::warn!(
                        path = ?ph.path,
                        aggregate_bytes,
                        per_image_bytes = loaded.data.len(),
                        cap = aggregate_max,
                        "placeholder_images: aggregate-bytes cap reached; skipping remaining orphan placeholders",
                    );
                    break;
                }
                aggregate_bytes = next_total;
                let data = base64::engine::general_purpose::STANDARD.encode(&loaded.data);
                raw_images.push(
                    agent_client_protocol::ImageContent::new(data, loaded.mime_type)
                        .uri(format!("file://{}", canonical.display()))
                        // Record the real `[Image #N]` number so it resolves by number TUI-attached images set it too.
                        .meta(display_number_meta(ph.display_number)),
                );
                recovered += 1;
            }
            Err(e) => {
                tracing::warn!(
                    path = ?ph.path,
                    error = %e,
                    "placeholder_images: orphan placeholder failed to load",
                );
            }
        }
    }
    recovered
}

/// Parse a `file://...` URI into a canonical `PathBuf`. Accepts both the relaxed unencoded form emitted by the TUI / server (see the module header) and the percent-encoded RFC 3986 form. Returns `None` if the URI does not start with `file://`; otherwise returns the canonicalised path. Falls back to the raw path when canonicalisation
/// fails so the caller can still compare against attached URIs.
pub fn canonical_from_file_uri(uri: &str) -> Option<PathBuf> {
    let raw_path_str = uri.strip_prefix("file://")?;
    // Try percent-decoding first; fall back to the literal form
    // Both are valid per the module's `file://` URI convention
    let decoded: std::borrow::Cow<'_, str> = match urlencoding::decode(raw_path_str) {
        Ok(c) => c,
        Err(_) => std::borrow::Cow::Borrowed(raw_path_str),
    };
    let raw = Path::new(decoded.as_ref());
    Some(dunce::canonicalize(raw).unwrap_or_else(|_| raw.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Minimal valid 1x1 PNG (real format, decodable by `image`).
    const PNG_BYTES: &[u8] = &[
        0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00, 0x00, 0x1F,
        0x15, 0xC4, 0x89, 0x00, 0x00, 0x00, 0x0D, 0x49, 0x44, 0x41, 0x54, 0x78, 0x9C, 0x63, 0x00,
        0x01, 0x00, 0x00, 0x05, 0x00, 0x01, 0x0D, 0x0A, 0x2D, 0xB4, 0x00, 0x00, 0x00, 0x00, 0x49,
        0x45, 0x4E, 0x44, 0xAE, 0x42, 0x60, 0x82,
    ];

    fn write_png(dir: &Path, name: &str) -> PathBuf {
        let path = dir.join(name);
        let mut f = std::fs::File::create(&path).unwrap();
        f.write_all(PNG_BYTES).unwrap();
        path
    }

    // ----- strip_paths_from_image_placeholders ---------------------------

    #[test]
    fn strip_paths_drops_path_keeps_anchor() {
        // The whole point: the model should see the bracketed anchor `[Image #N]` but not the path that would tempt a `Read`
        assert_eq!(
            strip_paths_from_image_placeholders(
                "what is that?[Image #1: /Users/me/Desktop/x.png] thanks".to_owned()
            ),
            "what is that?[Image #1] thanks"
        );
    }

    #[test]
    fn strip_paths_handles_multiple_placeholders_and_spaces_in_paths() {
        assert_eq!(
            strip_paths_from_image_placeholders(
                "[Image #1: /tmp/a.png] mid [Image #2: /home/u/My Pictures/b.jpg] tail".to_owned()
            ),
            "[Image #1] mid [Image #2] tail"
        );
    }

    #[test]
    fn strip_paths_returns_input_unchanged_when_no_placeholders() {
        // Fast path: the helper takes `String` by value and the no-match branch returns it verbatim, with no allocation
        let text = "no placeholder here, just prose";
        assert_eq!(strip_paths_from_image_placeholders(text.to_owned()), text);
    }

    #[test]
    fn strip_paths_preserves_surrounding_whitespace_and_unicode() {
        assert_eq!(
            strip_paths_from_image_placeholders(
                "café \u{202f}[Image #4: /Users/me/Desktop/Screenshot 2026-05-22 at 16.01.21.png] ok"
                    .to_owned()
            ),
            "café \u{202f}[Image #4] ok"
        );
    }

    #[test]
    fn strip_paths_ignores_malformed_placeholders() {
        let text = "[Image #1] [Image #2:] [Image #3: ] [Image #4: /ok.png";
        assert_eq!(strip_paths_from_image_placeholders(text.to_owned()), text);
    }

    // ----- extract_placeholders ------------------------------------------

    #[test]
    fn extract_placeholders_basic() {
        let text = "look at [Image #1: /tmp/a.png] and [Image #2: /home/user/b.jpg]";
        let matches = extract_placeholders(text);
        let [m0, m1] = matches.as_slice() else {
            panic!("expected two matches: {matches:?}");
        };
        assert_eq!(m0.display_number, 1);
        assert_eq!(m0.path, "/tmp/a.png");
        assert_eq!(m1.display_number, 2);
        assert_eq!(m1.path, "/home/user/b.jpg");
    }

    #[test]
    fn extract_placeholders_with_spaces_in_path() {
        let text = "[Image #3: /Users/me/My Pictures/cat.png]";
        let matches = extract_placeholders(text);
        let [m] = matches.as_slice() else {
            panic!("expected one match: {matches:?}");
        };
        assert_eq!(m.path, "/Users/me/My Pictures/cat.png");
    }

    #[test]
    fn extract_placeholders_rejects_partial_and_invalid_forms() {
        assert!(extract_placeholders("[Image #3] only").is_empty());
        assert!(extract_placeholders("[Image #4: ]").is_empty());
        assert!(extract_placeholders("[Image #5: /tmp/x.png").is_empty());
        assert!(extract_placeholders("[image #1: /tmp/x.png]").is_empty());
        assert!(extract_placeholders("[Image #: /tmp/x.png]").is_empty());
        // Missing space after colon: producer always emits ": " so the shorthand form is intentionally rejected
        assert!(extract_placeholders("[Image #5:foo.png]").is_empty());
    }

    #[test]
    fn extract_placeholders_inside_larger_text_returns_spans() {
        let text = "before [Image #7: /tmp/a.png] after";
        let matches = extract_placeholders(text);
        let [m] = matches.as_slice() else {
            panic!("expected one match: {matches:?}");
        };
        let (start, end) = m.span;
        assert_eq!(text.get(start..end), Some("[Image #7: /tmp/a.png]"));
    }

    #[test]
    fn extract_placeholders_unicode_path() {
        let text = "[Image #9: /tmp/café.png]";
        let matches = extract_placeholders(text);
        let [m] = matches.as_slice() else {
            panic!("expected one match: {matches:?}");
        };
        assert_eq!(m.path, "/tmp/café.png");
    }

    #[test]
    fn extract_placeholders_large_display_number() {
        let text = "[Image #99999: /tmp/a.png]";
        let matches = extract_placeholders(text);
        let [m] = matches.as_slice() else {
            panic!("expected one match: {matches:?}");
        };
        assert_eq!(m.display_number, 99999);
    }

    #[test]
    fn extract_placeholders_display_number_zero_is_kept() {
        let text = "[Image #0: /tmp/a.png]";
        let matches = extract_placeholders(text);
        let [m] = matches.as_slice() else {
            panic!("expected one match: {matches:?}");
        };
        assert_eq!(m.display_number, 0);
    }

    #[test]
    fn extract_placeholders_multiple_on_same_line() {
        let text = "[Image #1: /tmp/a.png] [Image #2: /tmp/b.png] [Image #3: /tmp/c.png]";
        let matches = extract_placeholders(text);
        let [m0, m1, m2] = matches.as_slice() else {
            panic!("expected three matches: {matches:?}");
        };
        assert_eq!(m0.display_number, 1);
        assert_eq!(m1.display_number, 2);
        assert_eq!(m2.display_number, 3);
    }

    #[test]
    fn extract_placeholders_at_text_boundaries() {
        let matches = extract_placeholders("[Image #1: /tmp/a.png] tail");
        let [m] = matches.as_slice() else {
            panic!("expected one match: {matches:?}");
        };
        assert_eq!(m.span.0, 0);
        let text2 = "head [Image #2: /tmp/b.png]";
        let matches2 = extract_placeholders(text2);
        let [m2] = matches2.as_slice() else {
            panic!("expected one match: {matches2:?}");
        };
        assert_eq!(m2.span.1, text2.len());
    }

    #[test]
    fn extract_placeholders_rejects_carriage_return_in_path() {
        let text = "[Image #1: /tmp/a\r.png]";
        assert!(extract_placeholders(text).is_empty());
    }

    #[test]
    fn extract_placeholders_caps_at_max_per_prompt() {
        let chunk = "[Image #1: /tmp/a.png] ".repeat(MAX_PLACEHOLDERS_PER_PROMPT + 5);
        let matches = extract_placeholders(&chunk);
        assert_eq!(matches.len(), MAX_PLACEHOLDERS_PER_PROMPT);
    }

    #[test]
    fn extract_placeholders_first_nested_bracket_terminates() {
        // Pinning behaviour.
        let text = "[Image #1: /tmp/[odd].png]";
        let matches = extract_placeholders(text);
        let [m] = matches.as_slice() else {
            panic!("expected one match: {matches:?}");
        };
        assert_eq!(m.path, "/tmp/[odd");
        let (start, end) = m.span;
        assert_eq!(text.get(start..end), Some("[Image #1: /tmp/[odd]"));
    }

    // ----- load_placeholder_image ----------------------------------------

    #[test]
    fn load_placeholder_image_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "ok.png");
        let canon = dunce::canonicalize(dir.path()).unwrap();
        let loaded =
            load_placeholder_image(path.to_str().unwrap(), std::slice::from_ref(&canon)).unwrap();
        assert_eq!(loaded.mime_type, "image/png");
        assert_eq!(loaded.data, PNG_BYTES);
    }

    #[test]
    fn load_placeholder_image_rejects_missing_path() {
        let dir = tempfile::tempdir().unwrap();
        let canon = dunce::canonicalize(dir.path()).unwrap();
        let missing = dir.path().join("does-not-exist.png");
        let err = load_placeholder_image(missing.to_str().unwrap(), std::slice::from_ref(&canon))
            .unwrap_err();
        assert!(matches!(err, PlaceholderLoadError::CanonicalizeFailed));
    }

    #[test]
    fn load_placeholder_image_rejects_unsupported_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("notes.txt");
        std::fs::write(&path, b"hello").unwrap();
        let canon = dunce::canonicalize(dir.path()).unwrap();
        let err = load_placeholder_image(path.to_str().unwrap(), std::slice::from_ref(&canon))
            .unwrap_err();
        assert!(matches!(err, PlaceholderLoadError::UnsupportedExtension));
    }

    #[test]
    fn load_placeholder_image_rejects_outside_allowed_prefixes() {
        let real_dir = tempfile::tempdir().unwrap();
        let png = write_png(real_dir.path(), "real.png");
        let other_dir = tempfile::tempdir().unwrap();
        let other_canon = dunce::canonicalize(other_dir.path()).unwrap();
        let err = load_placeholder_image(png.to_str().unwrap(), std::slice::from_ref(&other_canon))
            .unwrap_err();
        assert!(matches!(err, PlaceholderLoadError::OutsideAllowedPrefixes));
    }

    #[test]
    fn load_placeholder_image_rejects_not_an_image_with_image_extension() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("fake.png");
        std::fs::write(&path, b"not actually a png").unwrap();
        let canon = dunce::canonicalize(dir.path()).unwrap();
        let err = load_placeholder_image(path.to_str().unwrap(), std::slice::from_ref(&canon))
            .unwrap_err();
        assert!(matches!(err, PlaceholderLoadError::NotAnImage));
    }

    /// A file whose first several bytes are PNG magic but whose tail is arbitrary content (e.g. a private key) is **rejected** by the
    /// loader. `image::guess_format` alone accepted such forgeries; the full `with_guessed_format` and `into_dimensions` check closes that gap.
    #[test]
    fn load_placeholder_image_rejects_png_magic_forgery_with_garbage_tail() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forged.png");
        let mut bytes = vec![0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];
        bytes.extend(b"PRIVATE KEY DATA - not a real PNG");
        std::fs::write(&path, &bytes).unwrap();
        let canon = dunce::canonicalize(dir.path()).unwrap();
        let err = load_placeholder_image(path.to_str().unwrap(), std::slice::from_ref(&canon))
            .unwrap_err();
        assert!(
            matches!(err, PlaceholderLoadError::NotAnImage),
            "expected NotAnImage, got: {err:?}"
        );
    }

    #[test]
    fn load_placeholder_image_rejects_directory_as_not_a_file() {
        let allowed = tempfile::tempdir().unwrap();
        let dir_path = allowed.path().join("looks-like.png");
        std::fs::create_dir(&dir_path).unwrap();
        let canon = dunce::canonicalize(allowed.path()).unwrap();
        let err = load_placeholder_image(dir_path.to_str().unwrap(), std::slice::from_ref(&canon))
            .unwrap_err();
        assert!(matches!(err, PlaceholderLoadError::NotAFile));
    }

    #[cfg(unix)]
    #[test]
    fn load_placeholder_image_reports_read_failure_on_unreadable_file() {
        // SAFETY: getuid is always safe; we don't want to import libc.
        let euid = unsafe { libc::geteuid() };
        if euid == 0 {
            return;
        }
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("locked.png");
        std::fs::write(&path, PNG_BYTES).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o000)).unwrap();
        let canon = dunce::canonicalize(dir.path()).unwrap();
        let err = load_placeholder_image(path.to_str().unwrap(), std::slice::from_ref(&canon))
            .unwrap_err();
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        assert!(
            matches!(err, PlaceholderLoadError::ReadFailed(_)),
            "expected ReadFailed, got: {err:?}"
        );
    }

    #[test]
    fn load_placeholder_image_rejects_oversize_via_injectable_cap() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "small-but-over-cap.png");
        let canon = dunce::canonicalize(dir.path()).unwrap();
        let err = load_placeholder_image_with_cap(
            path.to_str().unwrap(),
            std::slice::from_ref(&canon),
            32,
        )
        .unwrap_err();
        match err {
            PlaceholderLoadError::TooLarge { actual, limit } => {
                assert_eq!(actual, PNG_BYTES.len());
                assert_eq!(limit, 32);
            }
            other => panic!("expected TooLarge, got {other:?}"),
        }
    }

    #[cfg(unix)]
    #[test]
    fn load_placeholder_image_rejects_symlink_escape() {
        let allowed_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let target = write_png(outside_dir.path(), "secret.png");
        let link = allowed_dir.path().join("link.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();

        let allowed_canon = dunce::canonicalize(allowed_dir.path()).unwrap();
        let err =
            load_placeholder_image(link.to_str().unwrap(), std::slice::from_ref(&allowed_canon))
                .unwrap_err();
        assert!(
            matches!(err, PlaceholderLoadError::OutsideAllowedPrefixes),
            "symlink escape must canonicalize then trip the prefix check, got: {err:?}",
        );
    }

    #[test]
    fn load_placeholder_image_rejects_path_traversal_via_dotdot() {
        let allowed_dir = tempfile::tempdir().unwrap();
        let outside_dir = tempfile::tempdir().unwrap();
        let unique = format!("ph-traversal-{}.png", uuid::Uuid::new_v4());
        let outside = outside_dir.path().join(&unique);
        std::fs::write(&outside, PNG_BYTES).unwrap();

        let traversal = allowed_dir
            .path()
            .join("..")
            .join(outside_dir.path().file_name().unwrap())
            .join(&unique);
        let allowed_canon = dunce::canonicalize(allowed_dir.path()).unwrap();
        let err = load_placeholder_image(
            traversal.to_str().unwrap(),
            std::slice::from_ref(&allowed_canon),
        )
        .unwrap_err();
        assert!(matches!(err, PlaceholderLoadError::OutsideAllowedPrefixes));
    }

    #[test]
    fn load_placeholder_image_rejects_deny_listed_subtree() {
        // Construct a workspace whose canonical contains `/.photoslibrary/` so the path is inside the allowlist but hits the deny-list
        let root = tempfile::tempdir().unwrap();
        let bundle = root
            .path()
            .join("Photos Library.photoslibrary")
            .join("originals");
        std::fs::create_dir_all(&bundle).unwrap();
        let png = write_png(&bundle, "hash.png");
        let allowed_canon = dunce::canonicalize(root.path()).unwrap();
        let err =
            load_placeholder_image(png.to_str().unwrap(), std::slice::from_ref(&allowed_canon))
                .unwrap_err();
        assert!(
            matches!(err, PlaceholderLoadError::OutsideAllowedPrefixes),
            "deny-listed subtree must be rejected even inside allowlist, got: {err:?}"
        );
    }

    // ----- default_allowed_prefixes / _with_home --------------------------

    #[test]
    fn default_allowed_prefixes_with_home_includes_workspace_and_every_subdir() {
        let dir = tempfile::tempdir().unwrap();
        let home = tempfile::tempdir().unwrap();
        // Create every entry in HOME_IMAGE_SUBDIRS so list drift is caught at test time, not at runtime
        let mut expected_subdir_canons: Vec<PathBuf> = Vec::new();
        for sub in HOME_IMAGE_SUBDIRS {
            let p = home.path().join(sub);
            std::fs::create_dir(&p).unwrap();
            expected_subdir_canons.push(dunce::canonicalize(&p).unwrap());
        }

        let prefixes =
            default_allowed_prefixes_with_home(dir.path(), Some(home.path().to_path_buf()));
        let dir_canon = dunce::canonicalize(dir.path()).unwrap();
        assert!(prefixes.contains(&dir_canon));
        for canon in &expected_subdir_canons {
            assert!(
                prefixes.contains(canon),
                "missing prefix for {canon:?} in {prefixes:?}"
            );
        }
        assert!(!prefixes.contains(&dunce::canonicalize(home.path()).unwrap()));
        // Uses `>=` not `==` so the test stays green if `$TMPDIR` resolves inside one of the home subdirs (e.g.
        // `TMPDIR=$HOME/Downloads/ci` on CI).
        assert!(
            prefixes.len() >= HOME_IMAGE_SUBDIRS.len(),
            "expected at least {} prefixes, got {prefixes:?}",
            HOME_IMAGE_SUBDIRS.len()
        );
    }

    #[test]
    fn default_allowed_prefixes_with_home_unset_returns_workspace_only() {
        let dir = tempfile::tempdir().unwrap();
        let prefixes = default_allowed_prefixes_with_home(dir.path(), None);
        let dir_canon = dunce::canonicalize(dir.path()).unwrap();
        assert_eq!(prefixes, vec![dir_canon]);
    }

    #[test]
    fn default_allowed_prefixes_dedups_workspace_equal_home_subdir() {
        let home = tempfile::tempdir().unwrap();
        let downloads = home.path().join("Downloads");
        std::fs::create_dir(&downloads).unwrap();
        let prefixes =
            default_allowed_prefixes_with_home(&downloads, Some(home.path().to_path_buf()));
        let dl_canon = dunce::canonicalize(&downloads).unwrap();
        let count = prefixes.iter().filter(|p| **p == dl_canon).count();
        assert_eq!(
            count, 1,
            "duplicate canonical prefix not deduped: {prefixes:?}"
        );
    }

    #[test]
    fn default_allowed_prefixes_drops_non_canonical_workspace() {
        let prefixes = default_allowed_prefixes_with_home(
            Path::new("/nonexistent/abs/path/we/never/created"),
            None,
        );
        assert!(
            prefixes.is_empty(),
            "non-canonical workspace must not be added to the allowlist: {prefixes:?}"
        );
    }

    // ----- canonical_from_file_uri ----------------------------------------

    #[test]
    fn canonical_from_file_uri_rejects_non_file_scheme() {
        assert!(canonical_from_file_uri("https://example.com/x.png").is_none());
        assert!(canonical_from_file_uri("/raw/path").is_none());
    }

    #[test]
    fn canonical_from_file_uri_handles_percent_encoded_path() {
        let dir = tempfile::tempdir().unwrap();
        let with_space = dir.path().join("My Pictures");
        std::fs::create_dir(&with_space).unwrap();
        let png = write_png(&with_space, "cat.png");
        let canon = dunce::canonicalize(&png).unwrap();
        // RFC 3986 form: spaces percent-encoded.
        let raw = format!("file://{}", png.display());
        let encoded = raw.replace(' ', "%20");
        let parsed = canonical_from_file_uri(&encoded).unwrap();
        assert_eq!(parsed, canon);
    }

    // ----- recover_orphan_placeholders (hermetic, no ambient $HOME) -------

    /// Build a non-empty ACP `ImageContent` so a future dedup change that short-circuits on `data.is_empty()` cannot silently pass these tests.
    fn make_acp_image(uri: &str) -> agent_client_protocol::ImageContent {
        agent_client_protocol::ImageContent::new("AAAA", "image/png").uri(Some(uri.to_string()))
    }

    #[test]
    fn recover_orphan_placeholders_loads_orphan() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "rec.png");
        let canon = dunce::canonicalize(&path).unwrap();
        let query = format!("look at [Image #1: {}]", canon.display());
        let mut raw: Vec<agent_client_protocol::ImageContent> = Vec::new();
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes(&query, &mut raw, &allowed);
        assert_eq!(n, 1);
        let [img] = raw.as_slice() else {
            panic!("expected one recovered image: {raw:?}");
        };
        assert_eq!(img.mime_type, "image/png");
        assert!(!img.data.is_empty());
        let Some(uri) = img.uri.as_deref() else {
            panic!("expected uri on recovered image");
        };
        assert!(uri.starts_with("file://"));
        // The recovered image carries its real `[Image #N]` number so `image_edit` can resolve the token to it by number
        assert_eq!(display_number_from_meta(img.meta.as_ref()), Some(1));
    }

    #[test]
    fn recover_orphan_placeholders_dedupes_against_canonical_uri() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_png(dir.path(), "dup.png");
        let canon = dunce::canonicalize(&path).unwrap();
        let attached_uri = format!("file://{}", canon.display());
        let mut raw = vec![make_acp_image(&attached_uri)];
        let query = format!("see [Image #1: {}]", canon.display());
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes(&query, &mut raw, &allowed);
        assert_eq!(n, 0, "canonical-canonical dedup must skip the load");
        assert_eq!(raw.len(), 1);
        // The entry must be untouched, not silently overwritten by a duplicate load
        assert_eq!(raw.first().map(|i| i.data.as_str()), Some("AAAA"));
    }

    #[cfg(unix)]
    #[test]
    fn recover_orphan_placeholders_dedupes_against_non_canonical_tui_uri() {
        let dir = tempfile::tempdir().unwrap();
        let outside_root = tempfile::tempdir().unwrap();
        let real_target = write_png(outside_root.path(), "data.png");
        let link = dir.path().join("link.png");
        std::os::unix::fs::symlink(&real_target, &link).unwrap();

        let attached_uri = format!("file://{}", link.display()); // non-canonical
        let mut raw = vec![make_acp_image(&attached_uri)];
        let canonical_placeholder = dunce::canonicalize(&real_target).unwrap();
        let query = format!("[Image #1: {}]", canonical_placeholder.display());
        let allowed = vec![dunce::canonicalize(outside_root.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes(&query, &mut raw, &allowed);
        assert_eq!(
            n, 0,
            "non-canonical TUI URI must still dedup against canonical placeholder path"
        );
        assert_eq!(raw.len(), 1);
        assert_eq!(raw.first().map(|i| i.data.as_str()), Some("AAAA"));
    }

    #[test]
    fn recover_orphan_placeholders_dedupes_against_percent_encoded_uri() {
        let dir = tempfile::tempdir().unwrap();
        let space_dir = dir.path().join("My Pictures");
        std::fs::create_dir(&space_dir).unwrap();
        let png = write_png(&space_dir, "cat.png");
        let canon = dunce::canonicalize(&png).unwrap();
        // Attached image URI uses RFC 3986 percent-encoded form.
        let raw_form = format!("file://{}", canon.display());
        let encoded = raw_form.replace(' ', "%20");
        let mut raw = vec![make_acp_image(&encoded)];
        let query = format!("[Image #1: {}]", canon.display());
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes(&query, &mut raw, &allowed);
        assert_eq!(
            n, 0,
            "percent-encoded `file://` URI must dedup against canonical placeholder"
        );
        assert_eq!(raw.len(), 1);
        assert_eq!(raw.first().map(|i| i.data.as_str()), Some("AAAA"));
    }

    /// Pin the inverse direction.
    /// The placeholder text wire format is the unencoded path produced by `xai_grok_pager::prompt_images::display_text`.
    /// A percent-encoded path *inside the placeholder text* is **not** supported.
    /// `extract_placeholders` captures the raw `%20` substring and `canonicalize` rejects the synthetic name.
    /// The orphan loader logs a warn and skips the placeholder.
    /// The already-attached canonical URI in `raw_images` stays untouched.
    #[test]
    fn recover_orphan_placeholders_percent_encoded_path_in_placeholder_is_not_supported() {
        let dir = tempfile::tempdir().unwrap();
        let space_dir = dir.path().join("My Pictures");
        std::fs::create_dir(&space_dir).unwrap();
        let png = write_png(&space_dir, "cat.png");
        let canon = dunce::canonicalize(&png).unwrap();
        let attached_uri = format!("file://{}", canon.display());
        let mut raw = vec![make_acp_image(&attached_uri)];
        // The placeholder carries a percent-encoded path, not the documented wire format
        let encoded_in_text = format!("{}", canon.display()).replace(' ', "%20");
        let query = format!("[Image #1: {}]", encoded_in_text);
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes(&query, &mut raw, &allowed);
        assert_eq!(n, 0);
        assert_eq!(raw.len(), 1, "attached URI must remain intact");
        assert_eq!(raw.first().map(|i| i.data.as_str()), Some("AAAA"));
    }

    #[test]
    fn recover_orphan_placeholders_zero_placeholders_short_circuits() {
        let dir = tempfile::tempdir().unwrap();
        let mut raw: Vec<agent_client_protocol::ImageContent> = Vec::new();
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes("just a message", &mut raw, &allowed);
        assert_eq!(n, 0);
        assert!(raw.is_empty());
    }

    #[test]
    fn recover_orphan_placeholders_failed_load_leaves_raw_images_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let missing = dir.path().join("nope.png");
        let mut raw: Vec<agent_client_protocol::ImageContent> = Vec::new();
        let query = format!("[Image #1: {}]", missing.display());
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes(&query, &mut raw, &allowed);
        assert_eq!(n, 0);
        assert!(raw.is_empty());
    }

    #[test]
    fn recover_orphan_placeholders_outside_allowlist_rejected() {
        let workspace = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        let png = write_png(outside.path(), "secret.png");
        let canon = dunce::canonicalize(&png).unwrap();
        let mut raw: Vec<agent_client_protocol::ImageContent> = Vec::new();
        let query = format!("[Image #1: {}]", canon.display());
        // Allowlist is workspace only; the placeholder canon is outside it
        let allowed = vec![dunce::canonicalize(workspace.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes(&query, &mut raw, &allowed);
        assert_eq!(n, 0);
        assert!(raw.is_empty());
    }

    // ----- Aggregate cap -------------------------------------------------

    /// Placeholders, aggregate cap below the cumulative byte total of both. The first
    /// image fits; the second pushes the running total over and the loop breaks.
    #[test]
    fn recover_orphan_placeholders_aggregate_cap_breaks_loop() {
        let dir = tempfile::tempdir().unwrap();
        let p1 = write_png(dir.path(), "a.png");
        let p2 = write_png(dir.path(), "b.png");
        let c1 = dunce::canonicalize(&p1).unwrap();
        let c2 = dunce::canonicalize(&p2).unwrap();
        let query = format!("[Image #1: {}] [Image #2: {}]", c1.display(), c2.display());
        let mut raw: Vec<agent_client_protocol::ImageContent> = Vec::new();
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        // Per-image cap permissive.
        let n = recover_orphan_placeholders_with_prefixes_and_caps(
            &query, &mut raw, &allowed, 1_000, 100,
        );
        assert_eq!(n, 1, "aggregate cap must allow exactly one image");
        assert_eq!(raw.len(), 1);
        // Order matters: the first placeholder's canonical URI is the one that landed in `raw`
        let Some(attached) = raw.first() else {
            panic!("expected attached image: {raw:?}");
        };
        let Some(attached_uri) = attached.uri.as_deref() else {
            panic!("expected uri on attached image");
        };
        assert!(
            attached_uri.contains("a.png"),
            "expected the first placeholder to be the one kept, got: {attached_uri}"
        );
    }

    /// Pin the aggregate-cap boundary: the cap is **inclusive**; `aggregate + image.len() > cap` triggers the break.
    /// A single image exactly equal to the cap is admitted.
    #[test]
    fn recover_orphan_placeholders_aggregate_cap_inclusive_boundary() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_png(dir.path(), "one.png");
        let c = dunce::canonicalize(&p).unwrap();
        let query = format!("[Image #1: {}]", c.display());
        let mut raw: Vec<agent_client_protocol::ImageContent> = Vec::new();
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        // The cap equals the image size, so the `>` comparison admits this image
        let n = recover_orphan_placeholders_with_prefixes_and_caps(
            &query,
            &mut raw,
            &allowed,
            1_000,
            PNG_BYTES.len(),
        );
        assert_eq!(n, 1);
        assert_eq!(raw.len(), 1);
    }

    /// The reject side of the inclusive-boundary contract.
    /// `cap == image_size - 1` is the smallest cap that rejects this image.
    #[test]
    fn recover_orphan_placeholders_aggregate_cap_inclusive_boundary_rejects_at_one_below() {
        let dir = tempfile::tempdir().unwrap();
        let p = write_png(dir.path(), "one.png");
        let c = dunce::canonicalize(&p).unwrap();
        let query = format!("[Image #1: {}]", c.display());
        let mut raw: Vec<agent_client_protocol::ImageContent> = Vec::new();
        let allowed = vec![dunce::canonicalize(dir.path()).unwrap()];
        let n = recover_orphan_placeholders_with_prefixes_and_caps(
            &query,
            &mut raw,
            &allowed,
            1_000,
            PNG_BYTES.len() - 1,
        );
        assert_eq!(n, 0, "cap one below image size must reject");
        assert!(raw.is_empty());
    }

    // ----- DENY_PATH_CONTAINS --------------------------------------------

    /// Every entry of `DENY_PATH_CONTAINS` produces an `OutsideAllowedPrefixes` rejection.
    /// The test loops the constant so a future PR that deletes a security-critical entry (e.g. `/.ssh/`) ships a failing test.
    #[test]
    fn load_placeholder_image_rejects_every_deny_list_entry() {
        // Build a canonical that contains each needle by constructing a directory tree under a tempdir
        // Each needle is wrapped in a leading `<allowed>` and a trailing `<file>.png`
        // Forward slashes only: the enforcement site normalises Windows backslashes to forward slashes before the substring check
        for needle in DENY_PATH_CONTAINS {
            let root = tempfile::tempdir().unwrap();
            let trimmed = needle.trim_matches('/');
            // Create the directory tree implied by the needle so canonicalize succeeds
            let mut current = root.path().to_path_buf();
            for segment in trimmed.split('/') {
                current = current.join(segment);
                std::fs::create_dir(&current).unwrap();
            }
            let png = write_png(&current, "x.png");
            let canon = dunce::canonicalize(&png).unwrap();
            // Allowlist is the root; without the deny-list, this path would be accepted
            let allowed = vec![dunce::canonicalize(root.path()).unwrap()];
            let err = load_placeholder_image(canon.to_str().unwrap(), &allowed).unwrap_err();
            assert!(
                matches!(err, PlaceholderLoadError::OutsideAllowedPrefixes),
                "needle {needle:?} did not produce OutsideAllowedPrefixes; got {err:?}"
            );
        }

        // Positive control.
        let root = tempfile::tempdir().unwrap();
        let png = write_png(root.path(), "picture.png");
        let canon = dunce::canonicalize(&png).unwrap();
        let allowed = vec![dunce::canonicalize(root.path()).unwrap()];
        let loaded =
            load_placeholder_image(canon.to_str().unwrap(), &allowed).unwrap_or_else(|e| {
                panic!("positive control: benign path inside allowed prefix must load, got: {e:?}")
            });
        // A regression that returned an empty `LoadedPlaceholderImage` would pass a bare `is_ok()` assertion Pin the mime type.
        assert_eq!(loaded.mime_type, "image/png");
        assert_eq!(loaded.data, PNG_BYTES);
    }

    #[test]
    fn attached_image_references_prefers_file_path_over_data() {
        // `[Image #N]` resolution should hand `image_edit` a bare on-disk path (from the durable `file://` URI)
        // `image_edit` then reads the session copy rather than re-decoding a large base64 blob
        let img = agent_client_protocol::ImageContent::new("AAAA", "image/png")
            .uri(Some(
                "file:///Users/me/.grok/sessions/s/images/image-1.png".into(),
            ))
            .meta(display_number_meta(1));
        let refs = attached_image_references(std::slice::from_ref(&img));
        assert_eq!(
            refs,
            vec![(
                1,
                "/Users/me/.grok/sessions/s/images/image-1.png".to_string()
            )]
        );
    }

    #[test]
    fn attached_image_references_falls_back_to_data_url() {
        // No durable URI (e.g. persistence failed): keep the inline bytes as a data URL so the token still resolves to the right image.
        let img = agent_client_protocol::ImageContent::new("BBBB", "image/jpeg")
            .meta(display_number_meta(2));
        let refs = attached_image_references(std::slice::from_ref(&img));
        assert_eq!(refs, vec![(2, "data:image/jpeg;base64,BBBB".to_string())]);
    }

    #[test]
    fn attached_image_references_keys_by_meta_number_not_position() {
        // Non-contiguous numbers (`#1`, `#3`) survive a mid-compose chip
        // removal The registry must key on the recorded number.
        let mk = |data: &str, n: usize| {
            agent_client_protocol::ImageContent::new(data, "image/png").meta(display_number_meta(n))
        };
        let refs = attached_image_references(&[mk("first", 1), mk("third", 3)]);
        assert_eq!(
            refs,
            vec![
                (1, "data:image/png;base64,first".to_string()),
                (3, "data:image/png;base64,third".to_string()),
            ]
        );
    }

    #[test]
    fn attached_image_references_falls_back_to_position_without_meta() {
        // Older client / non-TUI caller with no recorded number: fall back to 1-based position so the common contiguous case still resolves
        let mk = |data: &str| agent_client_protocol::ImageContent::new(data, "image/png");
        let refs = attached_image_references(&[mk("first"), mk("second")]);
        assert_eq!(
            refs,
            vec![
                (1, "data:image/png;base64,first".to_string()),
                (2, "data:image/png;base64,second".to_string()),
            ]
        );
    }
}
