#[derive(Clone, Copy)]
pub(crate) enum ShellRead {
    Read,
    Search,
    Reduce,
}

/// Incident: a dogfood session read a whole repository through `sed -n`, `head` and `grep -rn`, and
/// counted `grep -v test` lines as production, because a line filter drops lines holding a word.
pub(crate) fn shell_read_hint(command: &str) -> Option<String> {
    let segments: Vec<&str> = command
        .split(['|', ';', '\n'])
        .flat_map(|part| part.split("&&"))
        .filter(|part| !part.trim().is_empty())
        .collect();
    let mut target = None;
    for (index, segment) in segments.iter().enumerate() {
        match shell_read(segment)? {
            ShellRead::Reduce if index == 0 => return None,
            ShellRead::Reduce => {}
            ShellRead::Read => target = target.or(Some("read")),
            ShellRead::Search => target = target.or(Some("grep")),
        }
    }
    match target? {
        "grep" => Some(
            "[grep answers this: pattern=<regex> path=<dir> count for per-file hit counts; its hits \
carry the anchors an edit uses, and a `grep -v test` pipe counts lines holding the word, not test \
modules]"
                .to_owned(),
        ),
        _ => Some(
            "[read answers this: path=<file> offset=<line> limit=<n>; its rows carry the hashline \
anchors an edit uses, which a shell print of the file leaves out]"
                .to_owned(),
        ),
    }
}

fn shell_read(segment: &str) -> Option<ShellRead> {
    let words: Vec<&str> = segment.split_whitespace().collect();
    if words
        .iter()
        .any(|word| word.contains('>') || word.contains('`') || word.contains("$("))
    {
        return None;
    }
    let mut args = words.iter().skip_while(|word| word.contains('='));
    let verb = args.next()?.rsplit('/').next()?;
    let rest: Vec<&str> = args.copied().collect();
    let quiet = rest.iter().any(|word| {
        matches!(*word, "--quiet" | "--silent")
            || (word.starts_with('-') && !word.starts_with("--") && word.contains('n'))
    });
    match verb {
        "cat" | "head" | "tail" | "less" | "more" | "find" | "fd" | "tree" => Some(ShellRead::Read),
        "sed" if quiet => Some(ShellRead::Read),
        "grep" | "rg" | "ag" => Some(ShellRead::Search),
        "wc" | "sort" | "uniq" | "cut" => Some(ShellRead::Reduce),
        _ => None,
    }
}
