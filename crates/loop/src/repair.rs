/// Models reach for a name they half-remember — `Bash`, `functions.bash`,
/// `bash_tool` — with the call otherwise well formed, so a single unambiguous
/// match saves the turn a wasted round trip.
pub fn repair_tool_name<'a>(requested: &str, available: &[&'a str]) -> Option<&'a str> {
    if available.contains(&requested) {
        return None;
    }
    let wanted = normalize(requested);
    if wanted.is_empty() {
        return None;
    }
    let mut matched = None;
    for candidate in available {
        if normalize(candidate) == wanted {
            if matched.is_some() {
                return None;
            }
            matched = Some(*candidate);
        }
    }
    matched
}

/// Case, separators, a provider namespace prefix, and a `_tool`/`tool_` decoration
/// are all noise around the same name.
fn normalize(name: &str) -> String {
    let tail = name.rsplit(['.', ':']).next().unwrap_or(name);
    let folded: String = tail
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect();
    let without_suffix = folded.strip_suffix("tool").unwrap_or(&folded);
    without_suffix
        .strip_prefix("tool")
        .unwrap_or(without_suffix)
        .to_owned()
}
