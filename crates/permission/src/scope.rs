use std::path::Path;

use globset::{Glob, GlobBuilder, GlobSet, GlobSetBuilder};
use yi_types::permission::RuleKind;

use crate::catastrophic::{CatastrophicContext, absolute, lexical_normalize, resolve_links, word};
use crate::decide::ToolCall;
use crate::rules::{Grant, SessionRules, identity, tagged};

const SCOPE: &str = "write-scope";
pub(crate) struct RedirectTarget {
    pub(crate) path: String,
    pub(crate) directory_known: bool,
}
/// Incident: a character-boundary scan misread `%cd`, `grep cd` and `'cd'`; this reads shell words.
const DIRECTORY_WORDS: [&str; 3] = ["cd", "pushd", "popd"];
const COMMAND_OPENERS: [&str; 12] = [
    "if", "then", "elif", "else", "while", "until", "do", "!", "builtin", "command", "time", "}",
];
const WORD_BREAKS: &str = ";&|(){}<>`";
fn end_word(word: &mut String, command_start: &mut bool, moved: &mut bool) {
    if word.is_empty() {
        return;
    }
    *moved |= *command_start && DIRECTORY_WORDS.contains(&word.as_str());
    *command_start = COMMAND_OPENERS.contains(&word.as_str());
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
                    if !token.chars().all(|c| c.is_ascii_digit()) {
                        end_word(&mut token, &mut command_start, &mut moved);
                    }
                    token.clear();
                    let heredoc =
                        chars.next_if_eq(&'<').is_some() && chars.next_if_eq(&'<').is_none();
                    if heredoc {
                        chars.next_if_eq(&'-');
                        ends.extend(word(&mut chars));
                    }
                }
                (None, '>') => {
                    if !token.chars().all(|c| c.is_ascii_digit()) {
                        end_word(&mut token, &mut command_start, &mut moved);
                    }
                    token.clear();
                    while chars.next_if(|next| matches!(next, '>' | '|')).is_some() {}
                    let combined = chars.next_if_eq(&'&').is_some();
                    if let Some(target) = word(&mut chars)
                        && (!combined
                            || (target != "-" && !target.chars().all(|c| c.is_ascii_digit())))
                    {
                        let relative = !Path::new(&target).is_absolute();
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
fn compile(pattern: &str) -> Result<Glob, String> {
    if pattern.is_empty() || pattern.starts_with('/') {
        return Err(format!(
            "{pattern:?} is not relative to the working directory"
        ));
    }
    GlobBuilder::new(pattern)
        .literal_separator(true)
        .build()
        .map_err(|error| format!("invalid scope {pattern:?}: {error}"))
}

/// The brief of a run: every later write inside the working tree must fall under one scope
/// (a glob, relative to the tree) or be refused. A scope is kept as a command rule.
pub fn write_scope(pattern: &str) -> Result<Grant, String> {
    compile(pattern)?;
    Ok(Grant {
        kind: RuleKind::Command,
        canonical: identity(&[SCOPE, pattern]),
        label: format!("writes only under {pattern}"),
    })
}

/// Why this call may not run: it writes a path inside the working tree that no scope names.
/// A path is named when it, or a directory above it, matches. Outside the tree is the sandbox's.
pub(crate) fn outside(
    rules: &SessionRules,
    call: &ToolCall<'_>,
    context: &CatastrophicContext,
) -> Option<String> {
    let patterns: Vec<&str> = (rules.state().rules.iter())
        .filter_map(|rule| tagged(rule, SCOPE))
        .collect();
    if patterns.is_empty() || (call.reads_only && !call.irreversible) {
        return None;
    }
    let mut builder = GlobSetBuilder::new();
    for glob in patterns.iter().filter_map(|pattern| compile(pattern).ok()) {
        builder.add(glob);
    }
    // A scope that cannot compile names nothing, so an empty set refuses every write.
    let set = builder.build().unwrap_or_else(|_| GlobSet::empty());
    let root = resolve_links(&lexical_normalize(context.working_dir.as_deref()?));
    let redirected: Vec<_> = (call.command.into_iter())
        .flat_map(redirect_targets)
        .collect();
    let unreadable = (redirected.iter())
        .find(|target| !target.directory_known)
        .map(|target| target.path.clone());
    let redirected = redirected
        .into_iter()
        .filter(|target| target.directory_known)
        .map(|target| absolute(Path::new(&target.path), context));
    // A `grep` that applies a rewrite and names no path walks from the working directory.
    let walks = call.tool_name == "grep" && call.targets.is_empty();
    let written = (call.targets.iter())
        .map(|target| absolute(target, context))
        .chain(walks.then(|| root.clone()))
        .chain(redirected);
    for path in written {
        let real = resolve_links(&path);
        let Ok(relative) = real.strip_prefix(&root) else {
            continue;
        };
        let named = (relative.ancestors()).any(|dir| {
            !dir.as_os_str().is_empty() && (set.is_match(dir) || set.is_match(dir.join("")))
        });
        if !named {
            let shown = relative.to_string_lossy();
            return Some(format!(
                "{} is outside this run's write scope ({}). This is denied in every mode; leave it \
                 unchanged and say in your answer what the fix needs there.",
                if shown.is_empty() {
                    "the working directory"
                } else {
                    shown.as_ref()
                },
                patterns.join(", ")
            ));
        }
    }
    if let Some(unreadable) = unreadable {
        return Some(format!(
            "{unreadable} is redirected after a command that changes directory, so its path \
             cannot be read statically and cannot be shown to be inside this run's write scope \
             ({}). This is denied in every mode; give the redirect an absolute path, or take the \
             `cd` out of the command.",
            patterns.join(", ")
        ));
    }
    None
}
