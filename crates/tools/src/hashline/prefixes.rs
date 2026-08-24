use super::format::HL_FILE_HASH_LENGTH;

fn skip_ws(bytes: &[u8], mut index: usize) -> usize {
    while index < bytes.len() && (bytes[index] == b' ' || bytes[index] == b'\t') {
        index += 1;
    }
    index
}

fn scan_digits(bytes: &[u8], mut index: usize) -> Option<usize> {
    let start = index;
    while index < bytes.len() && bytes[index].is_ascii_digit() {
        index += 1;
    }
    (index > start).then_some(index)
}

/// `^\s*(?:>>>|>>)?\s*(?:[+*-]\s*)?\d+[:|]` — returns the byte length of the match.
fn hl_prefix_len(line: &str, require_plus: bool) -> Option<usize> {
    let bytes = line.as_bytes();
    let mut index = skip_ws(bytes, 0);
    if bytes[index..].starts_with(b">>>") {
        index += 3;
    } else if bytes[index..].starts_with(b">>") {
        index += 2;
    }
    index = skip_ws(bytes, index);
    let mut saw_plus = false;
    if index < bytes.len() && matches!(bytes[index], b'+' | b'*' | b'-') {
        saw_plus = bytes[index] == b'+';
        index += 1;
        index = skip_ws(bytes, index);
    }
    if require_plus && !saw_plus {
        return None;
    }
    index = scan_digits(bytes, index)?;
    if index < bytes.len() && (bytes[index] == b':' || (!require_plus && bytes[index] == b'|')) {
        return Some(index + 1);
    }
    None
}

fn is_hl_prefixed(line: &str) -> bool {
    hl_prefix_len(line, false).is_some()
}

fn is_plus_hl_prefixed(line: &str) -> bool {
    hl_prefix_len(line, true).is_some()
}

/// `^\s*\[[^#\r\n]+#hex{4}\]\s*$`
fn is_hashline_header_line(line: &str) -> bool {
    let trimmed = line.trim();
    let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    else {
        return false;
    };
    let Some(hash_at) = inner.rfind('#') else {
        return false;
    };
    let (path, hash) = inner.split_at(hash_at);
    let hash = &hash[1..];
    !path.is_empty()
        && !path.contains('#')
        && hash.len() == HL_FILE_HASH_LENGTH
        && hash.bytes().all(|byte| byte.is_ascii_hexdigit())
}

/// `^[+](?![+])`
fn is_diff_plus(line: &str) -> bool {
    let bytes = line.as_bytes();
    bytes.first() == Some(&b'+') && bytes.get(1) != Some(&b'+')
}

fn is_elision_marker(text: &str) -> bool {
    let trimmed = text.trim();
    trimmed == "\u{2026}" || trimmed == "..."
}

/// Display-only metadata emitted by `read`, never source: bracketed
/// truncation notices, range-elision rows (`5-9: … `), and bare `…` rows.
pub fn is_read_metadata_line(line: &str) -> bool {
    let trimmed = line.trim();
    if is_elision_marker(trimmed) {
        return true;
    }
    if let Some(inner) = trimmed
        .strip_prefix('[')
        .and_then(|rest| rest.strip_suffix(']'))
    {
        let showing = inner.starts_with("Showing lines ") && inner.contains(" of ");
        let more =
            inner.contains("more line") && (inner.contains("in file") || inner.contains("in "));
        let elided = inner.contains("ln elided") && inner.contains("re-read");
        if (showing || more) && inner.contains("Use :") {
            return true;
        }
        if elided {
            return true;
        }
    }
    let bytes = trimmed.as_bytes();
    if let Some(first_digits) = scan_digits(bytes, 0)
        && bytes
            .first()
            .is_some_and(|byte| (b'1'..=b'9').contains(byte))
    {
        let mut index = skip_ws(bytes, first_digits);
        if index < bytes.len() && bytes[index] == b'-' {
            index = skip_ws(bytes, index + 1);
            if let Some(second_digits) = scan_digits(bytes, index)
                && bytes.get(second_digits) == Some(&b':')
            {
                let rest = &trimmed[second_digits + 1..];
                return rest.contains('\u{2026}') || rest.contains("...");
            }
        }
    }
    false
}

fn strip_leading_hashline_prefixes(line: &str) -> String {
    let mut result = line;
    while let Some(len) = hl_prefix_len(result, false) {
        result = &result[len..];
    }
    result.to_owned()
}

pub fn strip_one_leading_hashline_prefix(line: &str) -> String {
    match hl_prefix_len(line, false) {
        Some(len) => line[len..].to_owned(),
        None => line.to_owned(),
    }
}

struct LinePrefixStats {
    non_empty: usize,
    header_count: usize,
    hash_prefix_count: usize,
    diff_plus_hash_prefix_count: usize,
    diff_plus_count: usize,
}

fn collect_line_prefix_stats(lines: &[String]) -> LinePrefixStats {
    let mut stats = LinePrefixStats {
        non_empty: 0,
        header_count: 0,
        hash_prefix_count: 0,
        diff_plus_hash_prefix_count: 0,
        diff_plus_count: 0,
    };
    for line in lines {
        if line.is_empty() {
            continue;
        }
        if is_read_metadata_line(line) {
            continue;
        }
        if is_hashline_header_line(line) {
            stats.non_empty += 1;
            stats.header_count += 1;
            continue;
        }
        stats.non_empty += 1;
        if is_hl_prefixed(line) {
            stats.hash_prefix_count += 1;
        }
        if is_plus_hl_prefixed(line) {
            stats.diff_plus_hash_prefix_count += 1;
        }
        if is_diff_plus(line) {
            stats.diff_plus_count += 1;
        }
    }
    stats
}

pub fn strip_new_line_prefixes(lines: Vec<String>) -> Vec<String> {
    let stats = collect_line_prefix_stats(&lines);
    if stats.non_empty == 0 {
        return lines;
    }
    let content_line_count = stats.non_empty - stats.header_count;
    let strip_hash = content_line_count > 0 && stats.hash_prefix_count == content_line_count;
    let strip_plus = !strip_hash
        && stats.diff_plus_hash_prefix_count == 0
        && stats.diff_plus_count > 0
        && stats.diff_plus_count * 2 >= stats.non_empty;
    if !strip_hash && !strip_plus && stats.diff_plus_hash_prefix_count == 0 {
        return lines;
    }
    lines
        .into_iter()
        .filter(|line| {
            !(is_read_metadata_line(line) || strip_hash && is_hashline_header_line(line))
        })
        .map(|line| {
            if strip_hash {
                strip_leading_hashline_prefixes(&line)
            } else if strip_plus {
                if is_diff_plus(&line) {
                    line[1..].to_owned()
                } else {
                    line
                }
            } else if stats.diff_plus_hash_prefix_count > 0 && is_plus_hl_prefixed(&line) {
                strip_one_leading_hashline_prefix(&line)
            } else {
                line
            }
        })
        .collect()
}

pub fn hashline_parse_text(edit: &str) -> Vec<String> {
    let trimmed = edit.strip_suffix('\n').unwrap_or(edit);
    let lines: Vec<String> = trimmed
        .replace('\r', "")
        .split('\n')
        .map(str::to_owned)
        .collect();
    strip_new_line_prefixes(lines)
}
