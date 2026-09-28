use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Anchor {
    pub line: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cursor {
    Bof,
    Eof,
    BeforeAnchor { anchor: Anchor },
    AfterAnchor { anchor: Anchor },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ParsedRange {
    pub start: Anchor,
    pub end: Anchor,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockMode {
    Replace,
    InsertAfter,
    Cut,
    PasteAfter,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Insert {
        cursor: Cursor,
        text: String,
        line_num: u64,
        index: usize,
        replacement: bool,
        block_start: Option<u64>,
    },
    Delete {
        anchor: Anchor,
        line_num: u64,
        index: usize,
        old_assertion: Option<String>,
    },
    Cut {
        range: ParsedRange,
        register: Option<String>,
        line_num: u64,
        index: usize,
    },
    Paste {
        at: PasteTarget,
        register: Option<String>,
        line_num: u64,
        index: usize,
        block_start: Option<u64>,
    },
    Block {
        anchor: Anchor,
        payloads: Vec<String>,
        mode: BlockMode,
        register: Option<String>,
        line_num: u64,
        index: usize,
    },
}

impl Edit {
    pub fn index(&self) -> usize {
        match self {
            Self::Insert { index, .. }
            | Self::Delete { index, .. }
            | Self::Cut { index, .. }
            | Self::Paste { index, .. }
            | Self::Block { index, .. } => *index,
        }
    }

    pub fn line_num(&self) -> u64 {
        match self {
            Self::Insert { line_num, .. }
            | Self::Delete { line_num, .. }
            | Self::Cut { line_num, .. }
            | Self::Paste { line_num, .. }
            | Self::Block { line_num, .. } => *line_num,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum PasteTarget {
    Gap { cursor: Cursor },
    Span { range: ParsedRange },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileOp {
    Rem,
    Move { dest: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct ApplyResult {
    pub text: String,
    /// Which source line each line of `text` came from; `None` for one the edit wrote.
    pub origins: Vec<Option<usize>>,
    pub first_changed_line: Option<u64>,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockSpan {
    pub start: u64,
    pub end: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockResolution {
    pub start: u64,
    pub end: u64,
    pub op: BlockMode,
}

pub struct BlockResolverRequest<'a> {
    pub path: &'a str,
    pub text: &'a str,
    pub line: u64,
}

pub type BlockResolver = dyn Fn(&BlockResolverRequest<'_>) -> Option<BlockSpan>;

/// The anonymous register is batch-local: it exists only between a CUT and a
/// paste inside one patch. Named/cross-batch registers are deferred (D35).
#[derive(Debug, Clone, Default)]
pub struct Clipboard {
    pub lines: Option<Vec<String>>,
    pub named: HashMap<String, Vec<String>>,
    pub pending_anon_cuts: Vec<String>,
}
