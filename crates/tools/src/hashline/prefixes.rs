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
