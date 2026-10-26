pub const HL_FILE_PREFIX: &str = "[";
pub const HL_FILE_SUFFIX: &str = "]";
pub const HL_PUT_KEYWORD: &str = "PUT";
pub const HL_CUT_KEYWORD: &str = "CUT";
pub const HL_REM_KEYWORD: &str = "REM";
pub const HL_MOVE_KEYWORD: &str = "MV";
pub const HL_HEADER_COLON: &str = ":";
pub const HL_FILE_HASH_SEP: &str = "#";
pub const HL_RANGE_SEP: &str = ".=";
pub const HL_LINE_BODY_SEP: &str = ":";
pub const HL_FILE_HASH_LENGTH: usize = 4;

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
