use std::sync::LazyLock;

use regex::bytes::{Captures, Regex};

/// Key-shaped tokens, named in the mark that replaces them. A value under a secret-looking name
/// needs 16 token characters and a digit, so `max_tokens = budget.remaining_tokens()` passes.
const SHAPES: [(&str, &str); 9] = [
    (
        "private-key",
        r"-----BEGIN [A-Z ]*PRIVATE KEY-----.*?-----END [A-Z ]*PRIVATE KEY-----",
    ),
    ("aws-key", r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b"),
    (
        "github-token",
        r"\b(?:gh[pousr]_[A-Za-z0-9]{36,}|github_pat_[A-Za-z0-9_]{40,})",
    ),
    ("api-key", r"\bsk-[A-Za-z0-9_-]{20,}"),
    ("slack-token", r"\bxox[abprs]-[A-Za-z0-9-]{10,}"),
    ("google-key", r"\bAIza[0-9A-Za-z_-]{35}"),
    (
        "jwt",
        r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
    ),
    (
        "authorization",
        r"(?i)\b(?:proxy-)?authorization\s*:\s*(?:(?:bearer|basic|token)\s+)?[A-Za-z0-9._~+/=-]{8,}",
    ),
    (
        "assigned-secret",
        r#"(?i)\b[A-Z0-9_]*(?:secret|token|passw(?:or)?d|api_?key|access_?key|private_?key)[A-Z0-9_]*["']?\s*[=:]\s*["']?(?P<value>[A-Za-z0-9_/+=-]{16,})"#,
    ),
];

static COMPILED: LazyLock<Option<Vec<(&'static str, Regex)>>> = LazyLock::new(|| {
    SHAPES
        .iter()
        .map(|(name, pattern)| Regex::new(pattern).ok().map(|regex| (*name, regex)))
        .collect()
});

/// A line longer than this is flushed in pieces, so a token can straddle the cut; a minified
/// bundle would otherwise be held in memory whole.
const LINE_CAP: usize = 1 << 20;

/// Secrets out of a byte stream before it is kept, a line at a time, so a kept file has the line
/// count the model saw and every `#L` pointer into it lands on the line it named.
pub struct Redactor {
    line: Vec<u8>,
    in_key: bool,
}

impl Redactor {
    /// `None` when a pattern fails to build: a caller then keeps nothing rather than keep secrets.
    pub fn new() -> Option<Self> {
        COMPILED.as_ref()?;
        Some(Self {
            line: Vec::new(),
            in_key: false,
        })
    }

    /// Redacted bytes for every line `bytes` completed; an open line waits for its newline.
    pub fn push(&mut self, bytes: &[u8], out: &mut Vec<u8>) {
        for chunk in bytes.split_inclusive(|byte| *byte == b'\n') {
            self.line.extend_from_slice(chunk);
            if self.line.ends_with(b"\n") || self.line.len() >= LINE_CAP {
                self.flush(out);
            }
        }
    }

    pub fn finish(mut self, out: &mut Vec<u8>) {
        self.flush(out);
    }

    fn flush(&mut self, out: &mut Vec<u8>) {
        let line = std::mem::take(&mut self.line);
        let (body, end) = match line.strip_suffix(b"\n") {
            Some(body) => (body, b"\n".as_slice()),
            None => (line.as_slice(), b"".as_slice()),
        };
        let key = has(body, b"PRIVATE KEY");
        let (begins, ends) = (
            key && has(body, b"-----BEGIN"),
            key && has(body, b"-----END"),
        );
        if self.in_key && !ends && armored(body) {
            out.extend_from_slice(&mark("private-key", body.len()));
        } else {
            self.in_key = begins && !ends;
            out.extend_from_slice(&redact_line(body));
        }
        out.extend_from_slice(end);
    }
}

/// A key block's body: base64 rows and `Name: value` headers. Any other line ends the block, so
/// a grep hit on a BEGIN line does not redact the rest of the output.
fn armored(line: &[u8]) -> bool {
    let line = line.strip_suffix(b"\r").unwrap_or(line);
    let base64 = |byte: &u8| byte.is_ascii_alphanumeric() || b"+/=".contains(byte);
    let header = line
        .iter()
        .position(|byte| *byte == b':')
        .is_some_and(|at| {
            let name = line.get(..at).unwrap_or_default();
            !name.is_empty() && name.iter().all(|b| b.is_ascii_alphanumeric() || *b == b'-')
        });
    line.iter().all(base64) || header
}

fn has(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

fn redact_line(line: &[u8]) -> Vec<u8> {
    let Some(shapes) = COMPILED.as_ref() else {
        return mark("unscanned", line.len());
    };
    let mut line = line.to_vec();
    for (name, regex) in shapes {
        if regex.is_match(&line) {
            let marked = |hit: &Captures<'_>| {
                let all = hit.get(0).map_or(&[][..], |all| all.as_bytes());
                let value = hit.name("value").map(|value| value.as_bytes());
                match value {
                    Some(value) if !value.iter().any(u8::is_ascii_digit) => all.to_vec(),
                    _ => mark(name, all.len()),
                }
            };
            line = regex.replace_all(&line, marked).into_owned();
        }
    }
    line
}

fn mark(name: &str, len: usize) -> Vec<u8> {
    format!("[redacted: {name}, {len} chars]").into_bytes()
}
