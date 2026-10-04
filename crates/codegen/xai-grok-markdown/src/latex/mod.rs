//! Best-effort conversion of LaTeX math to Unicode plain text.

mod commands;
mod cursor;
mod environments;
mod math_box;
mod symbols;

#[cfg(test)]
mod tests;

use commands::render_sequence;
use cursor::Cursor;
use math_box::MathBox;

/// Inputs larger than this are not converted (callers fall back to raw display).
pub(crate) const MAX_MATH_SOURCE_LEN: usize = 4096;

/// Hard cap on group-nesting recursion.
const MAX_DEPTH: usize = 32;

/// Convert inline math to a single-line Unicode string. Row separators (`\\`)
/// collapse to `; ` and multi-row environments render single-row, so inline
/// math never introduces a line break mid-paragraph.
pub(crate) fn latex_to_unicode_inline(src: &str) -> Option<String> {
    if src.len() > MAX_MATH_SOURCE_LEN {
        return None;
    }
    let lines = convert(src, true);
    let joined = lines
        .iter()
        .map(|l| l.trim())
        .filter(|l| !l.is_empty())
        .collect::<Vec<_>>()
        .join("; ");
    Some(joined)
}

/// Convert display math to one or more Unicode lines.
/// Leading whitespace is structural (box alignment) and preserved; only line ends are trimmed.
pub(crate) fn latex_to_unicode_display(src: &str) -> Option<Vec<String>> {
    if src.len() > MAX_MATH_SOURCE_LEN {
        return None;
    }
    let lines: Vec<String> = convert(src, false)
        .into_iter()
        .map(|l| l.trim_end().to_string())
        .filter(|l| !l.is_empty())
        .collect();
    Some(lines)
}

/// Run the converter and return the output lines.
fn convert(src: &str, flat: bool) -> Vec<String> {
    let mut cursor = Cursor::new(src);
    let mut out = MathBox::new(flat);
    render_sequence(&mut cursor, &mut out, 0, Mode::Math, None);
    out.into_lines()
}

/// Rendering mode: math mode applies typographic substitutions (`-` → `−`, `'` → `′`) that text fragments (`\text{...}`) must not receive.
#[derive(Copy, Clone, PartialEq, Eq)]
enum Mode {
    Math,
    Text,
}
