//! Golden test: `split_into_line_segments` against the output of the termwiz-based splitter it replaced.

use xai_ratatui_inline::split_into_line_segments;

const REFERENCE: &str = include_str!("fixtures/segment_reference.txt");

const WIDTHS: &[usize] = &[1, 2, 3, 5, 8, 10, 20, 80, 200];

const CORPUS: &[&str] = &[
    "",
    "hello",
    "hello world, this is a longer line that will wrap several times",
    "line1\nline2\nline3",
    "line1\r\nline2\r\n",
    "12345\r67",
    "\r\r\n\n\r",
    "😊😊😊 emoji wall 😊😊😊",
    "hello 你好 混合 width",
    "\x1b[31mred\x1b[0m plain \x1b[1;32;44mstyled\x1b[m",
    "\x1b[31m\x1b[1m\x1b[4mnested styles no text\x1b[0m",
    "12345678\x1b[0m90",
    "text\x1b]8;;https://example.com\x07link\x1b]8;;\x07 after",
    "osc title\x1b]0;window title\x07body",
    "cursor \x1b[2Amoves \x1b[10;20H everywhere",
    "tab\tand\x08backspace and \x07bell",
    "\x1b[38;5;196mext colors\x1b[38;2;10;20;30m truecolor\x1b[0m",
    "interrupted \x1b[3\nmid-sequence",
    "\x1b[31m\nstyle then newline",
    "trailing style 12345678\x1b[0m",
    "\x1b(Bcharset\x1b)0 escapes",
    "zero\u{200b}width\u{fe0f}chars",
    "combining a\u{0301}e\u{0301} accents",
];

const PIECES: &[&str] = &[
    "word",
    "a",
    "longer-token",
    " ",
    "  ",
    "\n",
    "\r",
    "\r\n",
    "😊",
    "你好",
    "é",
    "\u{200b}",
    "\x1b[31m",
    "\x1b[0m",
    "\x1b[1;44;38;5;10m",
    "\x1b[2K",
    "\x1b[10D",
    "\x1b]0;title\x07",
    "\x1b]8;;http://x\x07",
    "\t",
    "\x07",
];

fn all_cases() -> Vec<(String, usize)> {
    let mut cases = Vec::new();
    for input in CORPUS {
        for &w in WIDTHS {
            cases.push((input.to_string(), w));
        }
    }
    let mut state = 0x243F_6A88_85A3_08D3_u64; // seed: pi digits
    let mut next = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };
    for _ in 0..2000 {
        let mut input = String::new();
        let len = (next() % 30) as usize;
        for _ in 0..len {
            input.push_str(PIECES[(next() % PIECES.len() as u64) as usize]);
        }
        let width = 1 + (next() % 40) as usize;
        cases.push((input, width));
    }
    cases
}

fn encode(input: &str, width: usize) -> String {
    let base = input.as_ptr() as usize;
    let segs = split_into_line_segments(input, width)
        .iter()
        .map(|s| {
            let start = s.content.as_ptr() as usize - base;
            let crlf = if s.ends_with_crlf { "|" } else { "" };
            format!("{start}+{}{crlf}", s.content.len())
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!("{width}\t{segs}")
}

#[test]
fn splitter_matches_reference_output() {
    let cases = all_cases();
    let expected: Vec<&str> = REFERENCE.lines().collect();
    assert_eq!(
        expected.len(),
        cases.len(),
        "the fixture has one line per case"
    );
    for ((input, width), want) in cases.iter().zip(expected) {
        assert_eq!(
            encode(input, *width),
            want,
            "divergence for width {width}, input: {input:?}"
        );
    }
}
