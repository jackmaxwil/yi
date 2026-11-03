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
    let mut rows = Vec::new();
    let mut index = 0usize;
    for line in command.lines().filter(|line| !line.trim().is_empty()) {
        let Parsed::Segments(segments) = yi_permission::parse(line) else {
            return None;
        };
        for argv in &segments {
            let (assigned, verb) = crate::reduce::program(argv.iter().map(String::as_str))?;
            if yi_permission::classify(argv.get(assigned..)?) != Class::Safe {
                return None;
            }
            let rest: Vec<&str> = argv
                .get(assigned.saturating_add(1)..)?
                .iter()
                .map(String::as_str)
                .collect();
            match shell_read(verb, &rest) {
                Some(row) => rows.push(row),
                None if index > 0 && matches!(verb, "wc" | "sort" | "uniq" | "cut") => {}
                None => return None,
            }
            index = index.saturating_add(1);
        }
    }
    let row = if rows.contains(&GREP_ROW) {
        GREP_ROW
    } else {
        rows.first()?
    };
    Some(row.to_owned())
}

/// Invariant: a view the hashline bridge tagged already carries its edit anchor in the header
/// above, so the row claiming a shell print leaves anchors out would contradict it.
pub(crate) fn without_read_row(text: &str) -> String {
    text.split_inclusive('\n')
        .filter(|row| row.trim_end_matches('\n') != READ_ROW)
        .collect()
}

fn shell_read(verb: &str, rest: &[&str]) -> Option<&'static str> {
    let quiet = rest.iter().any(|word| {
        matches!(*word, "--quiet" | "--silent")
            || (word.starts_with('-') && !word.starts_with("--") && word.contains('n'))
    });
    match verb_category(verb, rest) {
        "search" => Some(GREP_ROW),
        "read" => match verb {
            "wc" | "file" | "stat" => None,
            "sed" if !quiet => None,
            _ => Some(READ_ROW),
        },
        _ => None,
    }
}
