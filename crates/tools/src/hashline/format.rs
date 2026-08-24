use super::types::Cursor;

pub const HL_FILE_PREFIX: &str = "[";
pub const HL_FILE_SUFFIX: &str = "]";
pub const HL_PAYLOAD_REPLACE: &str = "+";
pub const HL_PUT_KEYWORD: &str = "PUT";
pub const HL_CUT_KEYWORD: &str = "CUT";
pub const HL_REM_KEYWORD: &str = "REM";
pub const HL_MOVE_KEYWORD: &str = "MV";
pub const HL_HEADER_COLON: &str = ":";
pub const HL_GAP_BEFORE: &str = "<";
pub const HL_GAP_AFTER: &str = ">";
pub const HL_BLOCK_SUFFIX: &str = "*";
pub const HL_EOF_ANCHOR: &str = "$";
pub const HL_REGISTER_SIGIL: &str = "@";
pub const HL_FILE_HASH_SEP: &str = "#";
pub const HL_RANGE_SEP: &str = ".=";
pub const HL_LINE_BODY_SEP: &str = ":";
pub const HL_FILE_HASH_LENGTH: usize = 4;
pub const HL_FILE_HASH_EXAMPLES: [&str; 3] = ["1A2B", "3C4D", "9F3E"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct FileTag(pub u16);

impl std::fmt::Display for FileTag {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{:04X}", self.0)
    }
}

impl FileTag {
    pub fn parse(text: &str) -> Option<Self> {
        if text.len() != HL_FILE_HASH_LENGTH
            || !text
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_uppercase())
        {
            return None;
        }
        u16::from_str_radix(text, 16).ok().map(Self)
    }
}

fn normalize_file_hash_text(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for (index, line) in text.split('\n').enumerate() {
        if index > 0 {
            out.push('\n');
        }
        out.push_str(line.trim_end_matches([' ', '\t', '\r']));
    }
    out
}

pub fn compute_file_hash(text: &str) -> FileTag {
    let normalized = normalize_file_hash_text(text);
    FileTag((xxhash_rust::xxh32::xxh32(normalized.as_bytes(), 0) & 0xffff) as u16)
}

pub fn format_replace_header(start: u64, end: u64) -> String {
    format!("{HL_PUT_KEYWORD} {start}{HL_RANGE_SEP}{end}{HL_HEADER_COLON}")
}

pub fn format_cut_header(start: u64, end: u64) -> String {
    format!("{HL_CUT_KEYWORD} {start}{HL_RANGE_SEP}{end}")
}

pub fn format_gap_locator(cursor: &Cursor) -> String {
    match cursor {
        Cursor::BeforeAnchor { anchor } => format!("{HL_GAP_BEFORE}{}", anchor.line),
        Cursor::AfterAnchor { anchor } => format!("{HL_GAP_AFTER}{}", anchor.line),
        Cursor::Bof => format!("{HL_GAP_BEFORE}1"),
        Cursor::Eof => format!("{HL_GAP_AFTER}{HL_EOF_ANCHOR}"),
    }
}

pub fn format_insert_header(cursor: &Cursor) -> String {
    format!(
        "{HL_PUT_KEYWORD} {}{HL_HEADER_COLON}",
        format_gap_locator(cursor)
    )
}

pub fn format_register(name: &str) -> String {
    format!("{HL_REGISTER_SIGIL}{name}")
}

pub fn describe_anchor_examples(line_prefix: &str) -> String {
    let examples: Vec<String> = if line_prefix.is_empty() {
        vec!["160".to_owned(), "42".to_owned(), "7".to_owned()]
    } else {
        let stem = &line_prefix[..line_prefix.len().saturating_sub(1)];
        let second = if stem.is_empty() {
            "4".to_owned()
        } else {
            stem.to_owned()
        };
        vec![line_prefix.to_owned(), format!("{second}2"), "7".to_owned()]
    };
    examples
        .iter()
        .map(|example| format!("\"{example}\""))
        .collect::<Vec<_>>()
        .join(", ")
}

pub fn format_hashline_header(file_path: &str, file_hash: FileTag) -> String {
    format!("{HL_FILE_PREFIX}{file_path}{HL_FILE_HASH_SEP}{file_hash}{HL_FILE_SUFFIX}")
}

pub fn format_numbered_line(line_number: u64, line: &str) -> String {
    format!("{line_number}{HL_LINE_BODY_SEP}{line}")
}

pub fn split_addressable_file_lines(text: &str) -> Vec<&str> {
    let mut lines: Vec<&str> = text.split('\n').collect();
    if lines.len() > 1 && lines.last() == Some(&"") {
        lines.pop();
    }
    lines
}

pub fn format_numbered_lines(text: &str, start_line: u64) -> String {
    text.split('\n')
        .enumerate()
        .map(|(index, line)| format_numbered_line(start_line.saturating_add(index as u64), line))
        .collect::<Vec<_>>()
        .join("\n")
}
