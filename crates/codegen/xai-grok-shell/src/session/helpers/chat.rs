//! Shared text helper for the session summarizers and prompt builders.

/// Returns the largest valid UTF-8 character boundary index at or before `index`.
#[inline]
pub(super) fn floor_char_boundary(s: &str, index: usize) -> usize {
    if index >= s.len() {
        s.len()
    } else if s.is_char_boundary(index) {
        index
    } else {
        // UTF-8 characters are a bounded number of bytes, so the loop backs up a bounded number of bytes
        let mut i = index;
        while i > 0 && !s.is_char_boundary(i) {
            i -= 1;
        }
        i
    }
}
