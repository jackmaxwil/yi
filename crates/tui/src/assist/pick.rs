use std::path::Path;

use super::At;
use crate::commands::SLASH_COMMANDS;

const PICKER_CAP: usize = 5_000;
const SCHEMES: [&str; 5] = ["history://", "yi://", "kernel://", "local://", "family://"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Slash,
    Skill,
    File,
    Emoji,
    Scheme,
    Guess,
}

pub(super) struct Row {
    pub(super) label: String,
    pub(super) detail: String,
    pub(super) insert: String,
}

pub(super) struct Pick {
    pub(super) kind: Kind,
    pub(super) rows: Vec<Row>,
    pub(super) selected: usize,
    pub(super) start: usize,
    pub(super) end: usize,
    pub(super) token: String,
    pub(super) note: Option<String>,
}

/// The file walk and the skill list, read once per working directory.
#[derive(Default)]
pub(super) struct Sources {
    files: Option<(String, Vec<String>, bool)>,
    skills: Option<(String, Vec<(String, String)>)>,
}

impl Sources {
    pub(super) fn offer(&mut self, at: &At<'_>, line: &str) -> Option<Pick> {
        let (start, token) = token(line, at.col);
        let end = start.saturating_add(token.chars().count());
        let first = at.row == 0 && line.chars().take(start).all(char::is_whitespace);
        let (kind, from, rows, note) = if let Some(query) = token.strip_prefix('/') {
            let command = first && !query.starts_with('/') && !query.contains('/');
            (
                Kind::Slash,
                start,
                command.then(|| slash_rows(query))?,
                None,
            )
        } else if let Some(query) = token.strip_prefix('$') {
            let skills = self.skills(at.cwd);
            (Kind::Skill, start, named(skills, query, "$"), None)
        } else if let Some(at_sign) = token.chars().position(|ch| ch == '@') {
            let query: String = token.chars().skip(at_sign.saturating_add(1)).collect();
            let (files, capped) = self.files(at.cwd);
            let note = capped.then(|| {
                format!("[…] {PICKER_CAP} of {PICKER_CAP}+ files · picker cap {PICKER_CAP} · Esc, then type the path")
            });
            let rows = filter_files(files, &query).into_iter();
            let rows = rows
                .map(|path| Row::plain(&path, format!("{path} ")))
                .collect();
            (Kind::File, start.saturating_add(at_sign), rows, note)
        } else if let Some(query) = token.strip_prefix(':').filter(|q| q.chars().count() >= 2) {
            let rows =
                (EMOJI.iter()).filter(|(name, _)| name.starts_with(query) && !query.contains(':'));
            let rows = rows
                .map(|(name, emoji)| Row::plain(&format!(":{name}: {emoji}"), (*emoji).to_owned()));
            (Kind::Emoji, start, rows.collect(), None)
        } else {
            (Kind::Scheme, start, scheme_rows(&token), None)
        };
        (!rows.is_empty() || note.is_some()).then_some(Pick {
            kind,
            rows,
            selected: 0,
            start: from,
            end,
            token,
            note,
        })
    }

    fn files(&mut self, cwd: &str) -> (&[String], bool) {
        if self.files.as_ref().is_none_or(|(dir, _, _)| dir != cwd) {
            let mut files = crate::popup::walk_files(Path::new(cwd), PICKER_CAP + 1);
            let more = files.len() > PICKER_CAP;
            files.truncate(PICKER_CAP);
            self.files = Some((cwd.to_owned(), files, more));
        }
        (self.files.as_ref()).map_or((&[], false), |(_, files, more)| (files.as_slice(), *more))
    }

    fn skills(&mut self, cwd: &str) -> &[(String, String)] {
        if self.skills.as_ref().is_none_or(|(dir, _)| dir != cwd) {
            let home = crate::theme_file::home().unwrap_or_default();
            let found = yi_runtime::skills::discover(Path::new(cwd), &home).into_iter();
            let found = found.map(|skill| (skill.name, skill.description)).collect();
            self.skills = Some((cwd.to_owned(), found));
        }
        self.skills
            .as_ref()
            .map_or(&[], |(_, skills)| skills.as_slice())
    }
}

impl Row {
    fn plain(label: &str, insert: String) -> Self {
        Self {
            label: label.to_owned(),
            detail: String::new(),
            insert,
        }
    }
}

pub(super) fn guesses(words: Vec<String>, word: &str, col: usize, token: String) -> Option<Pick> {
    (!words.is_empty()).then(|| Pick {
        kind: Kind::Guess,
        rows: words
            .into_iter()
            .map(|guess| Row::plain(&guess, guess.clone()))
            .collect(),
        selected: 0,
        start: col.saturating_sub(word.chars().count()),
        end: col,
        token,
        note: None,
    })
}

/// Built-ins first, in table order with the exact name on top; a skill reaches a list through
/// `$`, so no skill can shadow a verb.
fn slash_rows(query: &str) -> Vec<Row> {
    named(
        &SLASH_COMMANDS.map(|(name, detail)| (name.to_owned(), detail.to_owned())),
        query,
        "/",
    )
}

fn named(table: &[(String, String)], query: &str, sigil: &str) -> Vec<Row> {
    let mut hits: Vec<(u8, &(String, String))> = (table.iter())
        .filter_map(|entry| {
            let name = entry.0.as_str();
            let rank = if name == query {
                0
            } else if name.starts_with(query) {
                1
            } else if name.contains(query) {
                2
            } else {
                return None;
            };
            Some((rank, entry))
        })
        .collect();
    hits.sort_by_key(|(rank, _)| *rank);
    let rows = hits.into_iter().map(|(_, (name, detail))| Row {
        label: format!("{sigil}{name}"),
        detail: detail.clone(),
        insert: format!("{sigil}{name} "),
    });
    rows.collect()
}

/// A scheme lists once its name is spelled up to a colon, so a prose word never opens one.
fn scheme_rows(token: &str) -> Vec<Row> {
    let Some((name, _)) = token.split_once(':') else {
        return Vec::new();
    };
    let spelled = |scheme: &&&str| {
        name.chars().count() >= 2
            && scheme.starts_with(name)
            && **scheme != token
            && (scheme.starts_with(token) || token == format!("{name}:"))
    };
    let hits = SCHEMES.iter().filter(spelled);
    hits.map(|scheme| Row::plain(scheme, (*scheme).to_owned()))
        .collect()
}

fn filter_files(names: &[String], query: &str) -> Vec<String> {
    let query_l = query.to_lowercase();
    let mut hits: Vec<&String> = (names.iter())
        .filter(|name| name.to_lowercase().contains(&query_l))
        .collect();
    let exact = |name: &str| name == query || name.rsplit('/').next() == Some(query);
    hits.sort_by_key(|name| !exact(name));
    hits.into_iter().cloned().collect()
}

/// The whitespace-delimited token around `col` and the char index it starts at.
pub(super) fn token(line: &str, col: usize) -> (usize, String) {
    let chars: Vec<char> = line.chars().collect();
    let (start, end) = super::mask::run(&chars, col, |ch| !ch.is_whitespace());
    (
        start,
        chars.get(start..end).unwrap_or_default().iter().collect(),
    )
}

pub(super) fn word_before(line: &str, col: usize) -> String {
    let chars: Vec<char> = line.chars().take(col).collect();
    let (start, end) = super::mask::run(&chars, col, char::is_alphabetic);
    chars.get(start..end).unwrap_or_default().iter().collect()
}

/// What `word` adds after `prefix`, matched without case, when it adds anything.
pub(super) fn suffix_of(word: &str, prefix: &str) -> Option<String> {
    let skip = prefix.chars().count();
    let head: String = word.chars().take(skip).collect();
    let rest: String = word.chars().skip(skip).collect();
    (head.to_lowercase() == prefix.to_lowercase() && !rest.is_empty()).then_some(rest)
}

pub(super) fn space_after(line: &str, col: usize) -> bool {
    line.chars()
        .nth(col)
        .is_none_or(|ch| !ch.is_whitespace() && !".,;:!?)]}'\"".contains(ch))
}

pub(super) fn emoji(name: &str) -> Option<&'static str> {
    EMOJI
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(_, emoji)| *emoji)
}

const EMOJI: &[(&str, &str)] = &[
    ("joy", "😂"),
    ("smile", "😄"),
    ("grin", "😁"),
    ("wink", "😉"),
    ("sob", "😭"),
    ("heart", "❤️"),
    ("thumbsup", "👍"),
    ("thumbsdown", "👎"),
    ("fire", "🔥"),
    ("rocket", "🚀"),
    ("eyes", "👀"),
    ("tada", "🎉"),
    ("check", "✅"),
    ("x", "❌"),
    ("warning", "⚠️"),
    ("star", "⭐"),
    ("wave", "👋"),
    ("clap", "👏"),
    ("ok_hand", "👌"),
    ("100", "💯"),
    ("thinking", "🤔"),
    ("sparkles", "✨"),
    ("zap", "⚡"),
    ("bug", "🐛"),
    ("memo", "📝"),
    ("bulb", "💡"),
];
