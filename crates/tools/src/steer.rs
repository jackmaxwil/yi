use crate::builtins::verb_category;
use yi_permission::{Class, Parsed};

const GREP_ROW: &str = "[grep answers this: pattern=<regex> path=<dir> count for per-file hit counts; its hits \
carry the anchors an edit uses, and a `grep -v test` pipe counts lines holding the word, not test \
modules]";

const READ_ROW: &str = "[read answers this: path=<file> offset=<line> limit=<n>; its rows carry the hashline \
anchors an edit uses, which a shell print of the file leaves out]";

/// Incident: a dogfood session read a whole repository through `sed -n`, `head` and `grep -rn`, and
/// counted `grep -v test` lines as production, because a line filter drops lines holding a word.
pub(crate) fn shell_read_hint(command: &str) -> Option<String> {
    let Parsed::Segments(segments) = yi_permission::parse(command) else {
        return None;
    };
    let mut rows = Vec::new();
    for (index, argv) in segments.iter().enumerate() {
        if yi_permission::classify(argv) != Class::Safe && !pager(argv) {
            return None;
        }
        let assigned = argv.iter().take_while(|word| word.contains('=')).count();
        let (verb, rest) = argv.get(assigned..)?.split_first()?;
        let verb = verb.rsplit('/').next()?;
        let rest: Vec<&str> = rest.iter().map(String::as_str).collect();
        match shell_read(verb, &rest) {
            Some(row) => rows.push(row),
            None if index > 0 && matches!(verb, "wc" | "sort" | "uniq" | "cut") => {}
            None => return None,
        }
    }
    let row = if rows.contains(&GREP_ROW) {
        GREP_ROW
    } else {
        rows.first()?
    };
    Some(row.to_owned())
}

fn pager(argv: &[String]) -> bool {
    matches!(
        argv.first().map(String::as_str),
        Some("less" | "more" | "ag")
    ) && argv
        .iter()
        .all(|word| !word.starts_with("--pager") && !word.starts_with('+'))
}

fn shell_read(verb: &str, rest: &[&str]) -> Option<&'static str> {
    let quiet = rest.iter().any(|word| {
        matches!(*word, "--quiet" | "--silent")
            || (word.starts_with('-') && !word.starts_with("--") && word.contains('n'))
    });
    match verb_category(verb, rest) {
        "search" => Some(GREP_ROW),
        "read" | "list_files" => match verb {
            "wc" | "file" | "stat" | "ls" => None,
            "sed" if !quiet => None,
            _ => Some(READ_ROW),
        },
        _ => None,
    }
}
