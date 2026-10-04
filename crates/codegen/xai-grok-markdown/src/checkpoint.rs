//! Checkpoint types for incremental markdown rendering.

/// A position in the source text where rendered content can be frozen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Checkpoint {
    /// Byte offset in source text (exclusive end of frozen region). Content in `text[..source_bytes]` can be cached.
    pub source_bytes: usize,
    /// Number of output lines that correspond to this checkpoint. Lines `0..output_lines` can be frozen.
    pub output_lines: usize,
    /// What kind of block ended at this checkpoint.
    pub kind: CheckpointKind,
}

/// The type of markdown block that created a checkpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CheckpointKind {
    /// A heading (any level: h1-h6)
    Heading,
    /// A paragraph followed by a blank line
    Paragraph,
    /// A fenced or indented code block
    CodeBlock,
    /// A blockquote that closed at top level
    BlockQuote,
    /// A list (ordered or unordered) that closed at top level
    List,
    /// A thematic break (horizontal rule: ---, ***, ___)
    ThematicBreak,
    /// A table that closed at top level
    Table,
    /// A raw HTML block
    HtmlBlock,
}
