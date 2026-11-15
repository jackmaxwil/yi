/// The hashline `[path#TAG]` header and the `NN:` row prefixes are anchors the
/// model edits against, not content the reader asked for.
pub(crate) fn strip_hashline(line: &str) -> &str {
    if line.starts_with('[') {
        return line;
    }
    numbered(line).map_or(line, |(_, text)| text)
}

/// `None` for a line with no number, which is how the hashline header and every
/// non-file tool fall through to a plain row.
pub(crate) fn numbered(line: &str) -> Option<(&str, &str)> {
    numbered_by(line, ':')
}

/// A grep context row, `19-text`: the same grammar as a hit with the other separator.
pub(crate) fn context_numbered(line: &str) -> Option<(&str, &str)> {
    numbered_by(line, '-')
}

fn numbered_by(line: &str, sep: char) -> Option<(&str, &str)> {
    let (number, text) = line.split_once(sep)?;
    (!number.is_empty() && number.bytes().all(|b| b.is_ascii_digit())).then_some((number, text))
}

/// Split so consecutive hits in one file print the path once, not once per row.
pub(crate) fn grep_row(line: &str) -> Option<(&str, &str, &str)> {
    if numbered(line).is_some() {
        return None;
    }
    pathed_row(line, ':')
}

/// The context-row twin of [`grep_row`], `path-19-text`, which grep prints once it stops
/// tagging files; a row that is tagless context already is not read as a path.
pub(crate) fn grep_context_row(line: &str) -> Option<(&str, &str, &str)> {
    if numbered(line).is_some() || context_numbered(line).is_some() {
        return None;
    }
    pathed_row(line, '-')
}

fn pathed_row(line: &str, sep: char) -> Option<(&str, &str, &str)> {
    let (at, _) = line.match_indices(sep).find(|(at, _)| {
        let rest = line.get(at.saturating_add(1)..).unwrap_or_default();
        let digits = rest.chars().take_while(|c| c.is_ascii_digit()).count();
        digits > 0 && rest.get(digits..).is_some_and(|tail| tail.starts_with(sep))
    })?;
    let path = line.get(..at)?;
    let rest = line.get(at.saturating_add(1)..)?;
    let (number, text) = rest.split_once(sep)?;
    Some((path, number, text))
}

/// `412 lines`, `3 hits`: a digest that is a count and its noun, nothing else.
pub(crate) fn counted(digest: &str) -> Option<(&str, &str)> {
    let (count, noun) = digest.split_once(' ')?;
    let numeric = !count.is_empty() && count.bytes().all(|b| b.is_ascii_digit());
    (numeric && !noun.is_empty() && !noun.contains(' ')).then_some((count, noun))
}
