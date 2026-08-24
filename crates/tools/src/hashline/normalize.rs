#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
}

pub fn detect_line_ending(content: &str) -> LineEnding {
    let crlf = content.find("\r\n");
    let lf = content.find('\n');
    match (crlf, lf) {
        (Some(crlf), Some(lf)) if crlf < lf => LineEnding::CrLf,
        _ => LineEnding::Lf,
    }
}

pub fn normalize_to_lf(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch == '\r' {
            if chars.peek() == Some(&'\n') {
                chars.next();
            }
            out.push('\n');
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn restore_line_endings(text: &str, ending: LineEnding) -> String {
    match ending {
        LineEnding::Lf => text.to_owned(),
        LineEnding::CrLf => text.replace('\n', "\r\n"),
    }
}

pub struct BomResult<'a> {
    pub bom: &'static str,
    pub text: &'a str,
}

pub fn strip_bom(content: &str) -> BomResult<'_> {
    content.strip_prefix('\u{FEFF}').map_or(
        BomResult {
            bom: "",
            text: content,
        },
        |text| BomResult {
            bom: "\u{FEFF}",
            text,
        },
    )
}
