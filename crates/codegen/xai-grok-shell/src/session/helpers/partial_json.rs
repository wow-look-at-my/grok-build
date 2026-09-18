//! Close a JSON document the model is still writing.
//!
//! A tool call's arguments arrive as fragments, and a fragment parses as
//! nothing. This turns the bytes seen so far into the largest valid document
//! they can stand for, so the fields that HAVE arrived can be read while the
//! rest is still on the wire.
//!
//! The completion never guesses at a value. It keeps what is complete, drops
//! the half-written tail, and appends the closers the open containers need.
/// An open container, and for an object whether the next string is a key.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Frame {
    Array,
    Object,
}
/// What the scanner is in the middle of at the current byte.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Tail {
    /// Between tokens. Nothing half-written.
    Between,
    /// Inside a string literal.
    Str,
    /// Inside a number that opened at this byte index.
    Number(usize),
    /// Inside a bare word (`true`, `false`, `null`) that opened here.
    Literal(usize),
}
/// The most complete valid JSON the fragment can stand for.
///
/// Returns `None` when nothing complete has arrived yet — an empty fragment, or
/// one that is only whitespace. A caller reads that as "no fields yet", which is
/// what `{}` would say anyway but without claiming a document exists.
///
/// The input is never required to be a prefix of valid JSON. Garbage in the tail
/// is dropped along with the rest of the half-written token.
pub fn complete_partial_json(input: &str) -> Option<String> {
    let bytes = input.as_bytes();
    let mut stack: Vec<Frame> = Vec::new();
    // The longest prefix that closing turns into a valid document, and the
    // containers open at its end. Both are only ever set together.
    let mut safe_len = 0usize;
    let mut safe_stack: Vec<Frame> = Vec::new();
    // Set when the scanner is inside a string: the last index at which the
    // string can be cut and closed. An escape in flight moves it forward only
    // once the escape is whole, so `"\u00` cuts back to before the backslash.
    let mut str_cut = 0usize;
    let mut str_is_key = false;
    let mut escaped = false;
    let mut unicode_left = 0u8;
    // Objects take a key next after `{` and after a `,`. Tracked apart from the
    // frame so a nested value can restore it on the way out.
    let mut expect_key = false;
    let mut tail = Tail::Between;
    let mut i = 0usize;
    while i < bytes.len() {
        let b = bytes[i];
        match tail {
            Tail::Str => {
                if unicode_left > 0 {
                    if b.is_ascii_hexdigit() {
                        unicode_left -= 1;
                        if unicode_left == 0 {
                            str_cut = i + 1;
                        }
                    } else {
                        // Not an escape the format allows. The string cannot be
                        // repaired past here, so stop growing the cut point.
                        unicode_left = 0;
                    }
                } else if escaped {
                    escaped = false;
                    if b == b'u' {
                        unicode_left = 4;
                    } else {
                        str_cut = i + 1;
                    }
                } else if b == b'\\' {
                    escaped = true;
                } else if b == b'"' {
                    tail = Tail::Between;
                    if str_is_key {
                        // A key alone cannot be closed over: `{"a"` is not a
                        // document. The safe point stays where the key started.
                    } else {
                        close_value(&mut stack, &mut expect_key);
                        mark_safe(i + 1, &stack, &mut safe_len, &mut safe_stack);
                    }
                } else {
                    str_cut = i + 1;
                }
            }
            Tail::Number(start) => {
                if is_number_byte(b) {
                    // Still in the number.
                } else {
                    if number_is_complete(&input[start..i]) {
                        close_value(&mut stack, &mut expect_key);
                        mark_safe(i, &stack, &mut safe_len, &mut safe_stack);
                    }
                    tail = Tail::Between;
                    continue;
                }
            }
            Tail::Literal(start) => {
                if b.is_ascii_alphabetic() {
                    // Still in the word.
                } else {
                    if matches!(&input[start..i], "true" | "false" | "null") {
                        close_value(&mut stack, &mut expect_key);
                        mark_safe(i, &stack, &mut safe_len, &mut safe_stack);
                    }
                    tail = Tail::Between;
                    continue;
                }
            }
            Tail::Between => match b {
                b'{' => {
                    stack.push(Frame::Object);
                    expect_key = true;
                    mark_safe(i + 1, &stack, &mut safe_len, &mut safe_stack);
                }
                b'[' => {
                    stack.push(Frame::Array);
                    expect_key = false;
                    mark_safe(i + 1, &stack, &mut safe_len, &mut safe_stack);
                }
                b'}' | b']' => {
                    stack.pop();
                    close_value(&mut stack, &mut expect_key);
                    mark_safe(i + 1, &stack, &mut safe_len, &mut safe_stack);
                }
                b'"' => {
                    tail = Tail::Str;
                    str_cut = i + 1;
                    str_is_key = matches!(stack.last(), Some(Frame::Object)) && expect_key;
                    escaped = false;
                    unicode_left = 0;
                }
                b':' => expect_key = false,
                b',' => {
                    if matches!(stack.last(), Some(Frame::Object)) {
                        expect_key = true;
                    }
                }
                b'-' | b'0'..=b'9' => tail = Tail::Number(i),
                b if b.is_ascii_alphabetic() => tail = Tail::Literal(i),
                _ => {}
            },
        }
        i += 1;
    }
    // The tail token, if it can stand on its own, extends the safe point.
    match tail {
        Tail::Number(start) if number_is_complete(&input[start..]) => {
            let mut ended = stack.clone();
            let mut ek = expect_key;
            close_value(&mut ended, &mut ek);
            mark_safe(input.len(), &ended, &mut safe_len, &mut safe_stack);
        }
        Tail::Literal(start) if matches!(&input[start..], "true" | "false" | "null") => {
            let mut ended = stack.clone();
            let mut ek = expect_key;
            close_value(&mut ended, &mut ek);
            mark_safe(input.len(), &ended, &mut safe_len, &mut safe_stack);
        }
        Tail::Str if !str_is_key => {
            // The string closes where it was last whole. Everything after that
            // is half an escape.
            let mut ended = stack.clone();
            let mut ek = expect_key;
            close_value(&mut ended, &mut ek);
            let mut out = String::with_capacity(str_cut + ended.len() + 1);
            out.push_str(&input[..str_cut]);
            out.push('"');
            push_closers(&mut out, &ended);
            return Some(out);
        }
        _ => {}
    }
    if safe_len == 0 {
        return None;
    }
    let mut out = String::with_capacity(safe_len + safe_stack.len());
    out.push_str(&input[..safe_len]);
    push_closers(&mut out, &safe_stack);
    Some(out)
}
/// Record a point the document can be closed at, with the containers open there.
fn mark_safe(len: usize, stack: &[Frame], safe_len: &mut usize, safe_stack: &mut Vec<Frame>) {
    *safe_len = len;
    safe_stack.clear();
    safe_stack.extend_from_slice(stack);
}
/// A value just ended: an object is back to wanting a key.
fn close_value(stack: &[Frame], expect_key: &mut bool) {
    *expect_key = matches!(stack.last(), Some(Frame::Object));
}
fn push_closers(out: &mut String, stack: &[Frame]) {
    for frame in stack.iter().rev() {
        out.push(match frame {
            Frame::Array => ']',
            Frame::Object => '}',
        });
    }
}
fn is_number_byte(b: u8) -> bool {
    b.is_ascii_digit() || matches!(b, b'-' | b'+' | b'.' | b'e' | b'E')
}
/// Whether the bytes so far are a whole JSON number. `12` is. `12.` and `1e` are
/// not, and closing over one of them writes a document no parser accepts.
fn number_is_complete(text: &str) -> bool {
    serde_json::from_str::<serde_json::Value>(text).is_ok()
}
#[cfg(test)]
mod tests {
    use super::complete_partial_json;
    /// Every prefix of a document completes to something a parser accepts. This
    /// is the whole contract: the fragment arrives a few bytes at a time and no
    /// arrival may produce a document that explodes.
    fn every_prefix_parses(doc: &str) {
        for end in 0..=doc.len() {
            if !doc.is_char_boundary(end) {
                continue;
            }
            let Some(completed) = complete_partial_json(&doc[..end]) else {
                continue;
            };
            serde_json::from_str::<serde_json::Value>(&completed).unwrap_or_else(|e| {
                panic!("prefix {end} of {doc:?} completed to {completed:?}, which is not JSON: {e}")
            });
        }
    }
    #[test]
    fn every_prefix_of_a_command_call_parses() {
        every_prefix_parses(r#"{"command": "ls -la /tmp", "is_background": false}"#);
    }
    #[test]
    fn every_prefix_of_a_nested_call_parses() {
        every_prefix_parses(
            r#"{"file_path":"/a/b.rs","edits":[{"old":"x","new":"y"},{"old":"1","new":"2"}],"n":-12.5e3}"#,
        );
    }
    #[test]
    fn every_prefix_of_an_escaped_body_parses() {
        every_prefix_parses(r#"{"content":"line\none\t\"quoted\" é \\ end","ok":true}"#);
    }
    #[test]
    fn a_value_string_is_closed_where_it_stands() {
        assert_eq!(
            complete_partial_json(r#"{"command": "ls -la"#).as_deref(),
            Some(r#"{"command": "ls -la"}"#)
        );
    }
    #[test]
    fn a_half_written_key_is_dropped_rather_than_guessed() {
        assert_eq!(complete_partial_json(r#"{"comm"#).as_deref(), Some("{}"));
        assert_eq!(
            complete_partial_json(r#"{"path":"/a","comm"#).as_deref(),
            Some(r#"{"path":"/a"}"#)
        );
    }
    #[test]
    fn a_key_awaiting_its_value_is_dropped() {
        assert_eq!(complete_partial_json(r#"{"path":"#).as_deref(), Some("{}"));
        assert_eq!(complete_partial_json(r#"{"path""#).as_deref(), Some("{}"));
    }
    #[test]
    fn a_dangling_comma_is_dropped() {
        assert_eq!(
            complete_partial_json(r#"{"a":1,"#).as_deref(),
            Some(r#"{"a":1}"#)
        );
        assert_eq!(complete_partial_json("[1, 2,").as_deref(), Some("[1, 2]"));
    }
    #[test]
    fn a_half_written_escape_is_cut_back_to_before_the_backslash() {
        assert_eq!(
            complete_partial_json(r#"{"s":"a\"#).as_deref(),
            Some(r#"{"s":"a"}"#)
        );
        assert_eq!(
            complete_partial_json(r#"{"s":"a\u00"#).as_deref(),
            Some(r#"{"s":"a"}"#)
        );
        assert_eq!(
            complete_partial_json(r#"{"s":"aé"#).as_deref(),
            Some(r#"{"s":"aé"}"#)
        );
    }
    #[test]
    fn a_complete_number_is_kept_and_a_half_written_one_is_not() {
        assert_eq!(
            complete_partial_json(r#"{"n":12"#).as_deref(),
            Some(r#"{"n":12}"#)
        );
        assert_eq!(complete_partial_json(r#"{"n":12."#).as_deref(), Some("{}"));
        assert_eq!(complete_partial_json(r#"{"n":-"#).as_deref(), Some("{}"));
        assert_eq!(complete_partial_json(r#"{"n":1e"#).as_deref(), Some("{}"));
    }
    #[test]
    fn a_half_written_literal_is_dropped() {
        assert_eq!(
            complete_partial_json(r#"{"a":1,"b":tr"#).as_deref(),
            Some(r#"{"a":1}"#)
        );
        assert_eq!(
            complete_partial_json(r#"{"a":1,"b":true"#).as_deref(),
            Some(r#"{"a":1,"b":true}"#)
        );
    }
    #[test]
    fn nesting_is_closed_innermost_first() {
        assert_eq!(
            complete_partial_json(r#"{"a":[{"b":"c"#).as_deref(),
            Some(r#"{"a":[{"b":"c"}]}"#)
        );
    }
    #[test]
    fn an_empty_or_blank_fragment_has_no_document() {
        assert_eq!(complete_partial_json(""), None);
        assert_eq!(complete_partial_json("   \n"), None);
    }
    #[test]
    fn a_complete_document_is_returned_unchanged() {
        let doc = r#"{"a":[1,2],"b":{"c":null}}"#;
        assert_eq!(complete_partial_json(doc).as_deref(), Some(doc));
    }
    #[test]
    fn a_multi_byte_character_split_by_a_fragment_does_not_panic() {
        every_prefix_parses(r#"{"s":"héllo wörld"}"#);
    }
    #[test]
    fn trailing_garbage_is_dropped_rather_than_carried() {
        assert_eq!(
            complete_partial_json(r#"{"a":1} !!"#).as_deref(),
            Some(r#"{"a":1}"#)
        );
    }
}
