/// What `split` took off a file so `restore` can put it back: its BOM and the terminator
/// that closed each source line, `"\n"`, `"\r\n"` or `"\r"`, in order.
#[derive(Debug, Clone)]
pub struct Endings {
    bom: &'static str,
    ends: Vec<&'static str>,
}

/// The LF text tools address, BOM off and every terminator read as one line break (a lone
/// CR is a line break, as `read` and `grep` count it), plus what restores it.
pub fn split(raw: &str) -> (String, Endings) {
    let BomResult { bom, text } = strip_bom(raw);
    let mut out = String::with_capacity(text.len());
    let mut ends = Vec::new();
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        let end = match ch {
            '\r' if chars.peek() == Some(&'\n') => {
                chars.next();
                "\r\n"
            }
            '\r' => "\r",
            '\n' => "\n",
            _ => {
                out.push(ch);
                continue;
            }
        };
        ends.push(end);
        out.push('\n');
    }
    (out, Endings { bom, ends })
}

impl Endings {
    /// `after` with the BOM and terminators back: output line `j` ends as source line
    /// `origin[j]` did; a line the edit wrote (`None`) ends as most lines do (tie: first seen).
    pub fn restore(&self, after: &str, origin: &[Option<usize>]) -> String {
        let mut tally: Vec<(&str, usize)> = Vec::new();
        for end in &self.ends {
            match tally.iter_mut().find(|(seen, _)| *seen == *end) {
                Some((_, count)) => *count += 1,
                None => tally.push((end, 1)),
            }
        }
        let majority = tally
            .iter()
            .fold(
                ("\n", 0),
                |best, &(end, count)| {
                    if count > best.1 { (end, count) } else { best }
                },
            )
            .0;
        let lines: Vec<&str> = after.split('\n').collect();
        let mut out = String::with_capacity(self.bom.len() + after.len() + lines.len());
        out.push_str(self.bom);
        for (index, line) in lines.iter().enumerate() {
            out.push_str(line);
            if index + 1 < lines.len() {
                let kept = origin
                    .get(index)
                    .copied()
                    .flatten()
                    .and_then(|source| self.ends.get(source));
                out.push_str(kept.copied().unwrap_or(majority));
            }
        }
        out
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
