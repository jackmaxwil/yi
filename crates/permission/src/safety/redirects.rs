pub(crate) struct RedirectTarget {
    pub(crate) path: String,
    pub(crate) directory_known: bool,
}
/// Incident: a character-boundary scan misread `%cd`, `grep cd` and `'cd'`; this reads shell words.
const DIRECTORY_WORDS: [&str; 3] = ["cd", "pushd", "popd"];
const WORD_BREAKS: &str = ";&|(){}`";
fn end_word(word: &mut String, command_start: &mut bool, moved: &mut bool) {
    if word.is_empty() {
        return;
    }
    *moved |= *command_start && DIRECTORY_WORDS.contains(&word.as_str());
    *command_start &= crate::catastrophic::wraps(word) && !matches!(word.as_str(), "-v" | "-V");
    word.clear();
}

fn end_before_redirect(word: &mut String, command_start: &mut bool, moved: &mut bool) {
    if !word.chars().all(|c| c.is_ascii_digit()) {
        end_word(word, command_start, moved);
    }
    word.clear();
}

/// The files a command redirects into; quotes, comments and heredoc bodies name nothing.
pub(crate) fn redirect_targets(command: &str) -> Vec<RedirectTarget> {
    let (mut files, mut quote, mut ends) = (Vec::new(), None, Vec::<String>::new());
    let (mut moved, mut command_start) = (false, true);
    let mut token = String::new();
    let mut continued = false;
    for line in command.split('\n') {
        let mut prev = ' ';
        let continuing = std::mem::take(&mut continued);
        if let Some(end) = ends.first() {
            if line.trim() == end {
                ends.remove(0);
            }
            token.clear();
            command_start = true;
            continue;
        }
        let mut chars = line.chars().peekable();
        while let Some(c) = chars.next() {
            match (quote, c) {
                (Some(open), c) if c == open => quote = None,
                (Some('"'), '\\') => match chars.next() {
                    Some(next) => token.push(next),
                    None => continued = true,
                },
                (Some(_), _) => token.push(c),
                (None, '\\') => match chars.next() {
                    Some(next) => token.push(next),
                    None => continued = true,
                },
                (None, '#') if prev.is_whitespace() => break,
                (None, '\'' | '"') => quote = Some(c),
                (None, '<') => {
                    end_before_redirect(&mut token, &mut command_start, &mut moved);
                    let heredoc =
                        chars.next_if_eq(&'<').is_some() && chars.next_if_eq(&'<').is_none();
                    if heredoc {
                        chars.next_if_eq(&'-');
                        ends.extend(crate::catastrophic::word(&mut chars));
                    }
                }
                (None, '>') => {
                    end_before_redirect(&mut token, &mut command_start, &mut moved);
                    while chars.next_if(|next| matches!(next, '>' | '|')).is_some() {}
                    let combined = chars.next_if_eq(&'&').is_some();
                    if let Some(target) = crate::catastrophic::word(&mut chars)
                        && (!combined
                            || (target != "-" && !target.chars().all(|c| c.is_ascii_digit())))
                    {
                        let relative = !std::path::Path::new(&target).is_absolute();
                        files.push(RedirectTarget {
                            path: target,
                            directory_known: !(moved && relative),
                        });
                    }
                }
                (None, c) if WORD_BREAKS.contains(c) => {
                    end_word(&mut token, &mut command_start, &mut moved);
                    command_start = true;
                }
                (None, c) if c.is_whitespace() => {
                    end_word(&mut token, &mut command_start, &mut moved);
                }
                (None, c) => token.push(c),
            }
            prev = c;
        }
        if !continuing {
            end_word(&mut token, &mut command_start, &mut moved);
            command_start = true;
        }
    }
    files
}
