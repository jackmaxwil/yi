use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};

use crate::commands::SLASH_COMMANDS;
use yi_types::config::{AssistSettings, CompleteEngine};

mod engine;
mod mask;

use engine::{
    add_words, apple_correct, apple_engine, apple_guesses, apple_suffix, apple_typos, autocorrect,
    fake, force_native, guard, ledger_for, ledger_for_key, ngram_suffix, settings, smol_engine,
    typos_on,
};
use mask::{collapse, keep_span, prose_mask, triple_spans};

/// Invariant: the error color is the underline, and the letters keep the color they had.
pub fn spell_style(color: Color) -> Style {
    Style::default()
        .underline_color(color)
        .add_modifier(Modifier::UNDERLINED)
}

const PICKER_CAP: usize = 5_000;
const CAP_NOTE: &str = "[…] 5000 of 5000+ files · picker cap 5000 · Esc, then type the path";
const SCHEMES: [&str; 5] = ["history://", "yi://", "kernel://", "local://", "family://"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharSpan {
    pub start: usize,
    pub end: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Plain,
    Typo,
    Ghost,
}

pub struct FileOffer {
    pub labels: Vec<String>,
    pub searching: bool,
    pub note: Option<String>,
}

enum OverrideKind {
    Fake,
    Auto,
    Words,
}

pub struct Override {
    kind: OverrideKind,
}

impl Drop for Override {
    fn drop(&mut self) {
        match self.kind {
            OverrideKind::Fake => FAKE.with(|flag| flag.set(false)),
            OverrideKind::Auto => AUTO.with(|flag| flag.set(None)),
            OverrideKind::Words => EXTRA.with(|words| words.borrow_mut().clear()),
        }
    }
}

thread_local! {
    static FAKE: Cell<bool> = const { Cell::new(false) };
    static AUTO: Cell<Option<bool>> = const { Cell::new(None) };
    static EXTRA: RefCell<HashMap<String, u32>> = RefCell::new(HashMap::new());
    static FORCE: Cell<bool> = const { Cell::new(false) };
}

static ARMED: AtomicBool = AtomicBool::new(false);
static SETTINGS: Mutex<AssistSettings> = Mutex::new(AssistSettings::DEFAULT);
static LEDGER: Mutex<Option<Ledger>> = Mutex::new(None);

struct Ledger {
    key: String,
    words: HashMap<String, u32>,
    slashes: HashMap<String, u32>,
}

pub fn install(settings: AssistSettings) {
    *guard(&SETTINGS) = settings;
    ARMED.store(true, Ordering::Relaxed);
}

pub fn fake_engine() -> Override {
    FAKE.with(|flag| flag.set(true));
    Override {
        kind: OverrideKind::Fake,
    }
}

pub fn autocorrect_on() -> Override {
    AUTO.with(|flag| flag.set(Some(true)));
    Override {
        kind: OverrideKind::Auto,
    }
}

pub fn observe(text: &str) -> Override {
    let mut words = EXTRA.with(|slot| slot.borrow().clone());
    add_words(&mut words, text);
    EXTRA.with(|slot| *slot.borrow_mut() = words);
    Override {
        kind: OverrideKind::Words,
    }
}

pub fn char_index(text: &str, utf16: usize) -> usize {
    let mut units = 0;
    for (index, ch) in text.chars().enumerate() {
        if units >= utf16 {
            return index;
        }
        units += ch.len_utf16();
    }
    text.chars().count()
}

pub fn file_rows(
    previous: &[String],
    fresh: Option<&[String]>,
    query: &str,
    capped: bool,
    limit: usize,
) -> FileOffer {
    let (source, searching) = match fresh {
        Some(rows) => (rows, false),
        None => (previous, true),
    };
    FileOffer {
        labels: filter_files(source, query, limit),
        searching,
        note: capped.then(|| CAP_NOTE.to_owned()),
    }
}

pub fn blend(apple: Option<&str>, ngram: Option<&str>, smol: Option<&str>) -> Option<String> {
    let present: Vec<&str> = [apple, ngram, smol]
        .into_iter()
        .flatten()
        .filter(|suffix| !suffix.is_empty())
        .collect();
    for longer in &present {
        for shorter in &present {
            if longer.len() > shorter.len() && longer.starts_with(shorter) {
                return Some((*longer).to_owned());
            }
        }
    }
    present.first().map(|suffix| (*suffix).to_owned())
}

pub fn in_typing_word(line: &str, cursor: usize, cell: usize) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let cursor = cursor.min(chars.len());
    let letter = |index: usize| {
        chars
            .get(index)
            .is_some_and(|ch| ch.is_alphabetic() || *ch == '\'')
    };
    let anchor = if letter(cursor) {
        cursor
    } else if cursor > 0 && letter(cursor - 1) {
        cursor - 1
    } else {
        return false;
    };
    let mut start = anchor;
    while start > 0 && letter(start - 1) {
        start -= 1;
    }
    let mut end = anchor + 1;
    while end < chars.len() && letter(end) {
        end += 1;
    }
    cell >= start && cell < end
}

pub fn typo_spans(text: &str) -> Vec<CharSpan> {
    if !typos_on() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let mask = prose_mask(&chars);
    let mut spans = if fake() {
        triple_spans(&chars)
    } else {
        apple_typos(text)
    };
    spans.retain(|span| keep_span(&chars, &mask, span.start, span.end));
    spans
}

pub fn ghost_suffix(line: &str, col: usize) -> String {
    let prefix = word_before(line, col);
    if prefix.chars().count() < 2 {
        return String::new();
    }
    match settings().complete {
        CompleteEngine::Off => String::new(),
        CompleteEngine::Ngram => ngram_suffix(&prefix).unwrap_or_default(),
        CompleteEngine::Apple => apple_suffix(&prefix).unwrap_or_default(),
        CompleteEngine::Smol => String::new(),
        CompleteEngine::Auto => blend(
            apple_suffix(&prefix).as_deref(),
            ngram_suffix(&prefix).as_deref(),
            None,
        )
        .unwrap_or_default(),
    }
}

pub fn tones(line: &str, col: usize) -> Vec<(String, Tone)> {
    let spans = typo_spans(line);
    let mut out = Vec::new();
    let mut buf = String::new();
    let mut tone = Tone::Plain;
    for (index, ch) in line.chars().enumerate() {
        let next = if spans
            .iter()
            .any(|span| span.start <= index && index < span.end)
            && !in_typing_word(line, col, index)
        {
            Tone::Typo
        } else {
            Tone::Plain
        };
        if next != tone && !buf.is_empty() {
            out.push((std::mem::take(&mut buf), tone));
        }
        tone = next;
        buf.push(ch);
    }
    if !buf.is_empty() {
        out.push((buf, tone));
    }
    let ghost = ghost_suffix(line, col);
    if !ghost.is_empty() {
        out.push((ghost, Tone::Ghost));
    }
    out
}

pub fn rewrite_emoji(line: &str, col: usize) -> Option<(String, usize)> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = String::new();
    let mut shifted = col;
    let mut i = 0;
    let mut changed = false;
    while i < chars.len() {
        if chars[i] == ':'
            && let Some((name, end)) = shortcode(&chars, i)
            && let Some(emoji) = emoji(name)
        {
            if i < col {
                let removed = end - i;
                let added = emoji.chars().count();
                if col < end {
                    shifted = out.chars().count() + added;
                } else {
                    shifted = shifted - removed + added;
                }
            }
            out.push_str(emoji);
            i = end;
            changed = true;
            continue;
        }
        out.push(chars[i]);
        i += 1;
    }
    changed.then_some((out, shifted))
}

pub fn correct_last(line: &str, col: usize) -> Option<(String, usize)> {
    if !autocorrect() {
        return None;
    }
    let chars: Vec<char> = line.chars().collect();
    let col = col.min(chars.len());
    let mut start = col;
    while start > 0 && chars[start - 1].is_alphabetic() {
        start -= 1;
    }
    if start == col {
        return None;
    }
    let word: String = chars[start..col].iter().collect();
    let fixed = if fake() {
        collapse(&word)?
    } else {
        apple_correct(&word)?
    };
    if fixed == word {
        return None;
    }
    let mut out = String::new();
    out.extend(chars[..start].iter());
    out.push_str(&fixed);
    out.extend(chars[col..].iter());
    Some((out, start + fixed.chars().count()))
}

pub fn guesses_for(line: &str, col: usize) -> Vec<String> {
    let word = word_before(line, col);
    if word.is_empty() {
        return Vec::new();
    }
    if fake() {
        return collapse(&word).into_iter().collect();
    }
    apple_guesses(&word)
}

pub fn report(home: &Path, prefix: &str) -> i32 {
    let _force = force_native();
    let prefix = if prefix.is_empty() { "hel" } else { prefix };
    let _ = ledger_for(&home.join(".yi/sessions"));
    let ngram = ngram_suffix(prefix)
        .map(Engine::Hit)
        .unwrap_or(Engine::Miss);
    let apple = apple_engine(prefix);
    let smol = smol_engine(home, prefix);
    let blended = blend(hit(&apple), hit(&ngram), hit(&smol));
    println!("ngram: {}", show(&ngram));
    println!("apple: {}", show(&apple));
    println!("smol: {}", show(&smol));
    println!("auto: {}", blended.unwrap_or_else(|| "none".to_owned()));
    0
}

enum Engine {
    Absent,
    Miss,
    Hit(String),
}

fn hit(engine: &Engine) -> Option<&str> {
    match engine {
        Engine::Hit(text) => Some(text.as_str()),
        _ => None,
    }
}

fn show(engine: &Engine) -> String {
    match engine {
        Engine::Absent => "absent".to_owned(),
        Engine::Miss => "none".to_owned(),
        Engine::Hit(text) => text.clone(),
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Slash,
    File,
    Emoji,
    Scheme,
    Guess,
}

struct Row {
    label: String,
    detail: String,
    insert: String,
}

struct Pick {
    kind: Kind,
    rows: Vec<Row>,
    selected: usize,
    start: usize,
    end: usize,
    token: String,
    note: Option<String>,
    searching: bool,
}

pub struct Assist {
    typos: Vec<(usize, usize, usize)>,
    cache: Vec<(String, Vec<(usize, usize)>)>,
    ghost: String,
    pick: Option<Pick>,
    dismissed: Option<String>,
    files: Option<(String, Vec<String>, bool)>,
    skills: Option<(String, Vec<(String, String)>)>,
    typo: Style,
    dim: Style,
}

pub enum Act {
    Nope,
    Insert(String),
    Splice {
        start: usize,
        end: usize,
        text: String,
    },
}

impl Assist {
    pub fn new() -> Self {
        Self {
            typos: Vec::new(),
            cache: Vec::new(),
            ghost: String::new(),
            pick: None,
            dismissed: None,
            files: None,
            skills: None,
            typo: spell_style(Color::Rgb(255, 95, 95)),
            dim: Style::default().add_modifier(Modifier::DIM),
        }
    }

    pub fn set_typo(&mut self, style: Style) {
        self.typo = style;
    }

    pub fn set_dim(&mut self, style: Style) {
        self.dim = style;
    }

    pub fn typo_style(&self) -> Style {
        self.typo
    }

    pub fn dim_style(&self) -> Style {
        self.dim
    }

    pub fn ghost(&self) -> &str {
        &self.ghost
    }

    pub fn open(&self) -> bool {
        self.pick.is_some()
    }

    pub fn typo_at(&self, line: usize, col: usize) -> bool {
        self.typos
            .iter()
            .any(|&(at, start, end)| at == line && start <= col && col < end)
    }

    pub fn sync(
        &mut self,
        text: &str,
        line: &str,
        _line_index: usize,
        col: usize,
        cwd: &str,
        session_dir: &str,
    ) {
        self.typos = self.mark(text);
        self.ghost = ghost_suffix(line, col);
        let token = token_at(line, col);
        if self.dismissed.as_ref().is_some_and(|held| held == &token) {
            self.pick = self.pick.take().filter(|pick| pick.kind == Kind::Guess);
            return;
        }
        self.dismissed = None;
        if self
            .pick
            .as_ref()
            .is_some_and(|pick| pick.kind == Kind::Guess)
        {
            return;
        }
        self.pick = self.offer(line, col, &token, cwd, session_dir);
    }

    pub fn lines(&self, width: usize) -> Option<Vec<Line<'static>>> {
        let pick = self.pick.as_ref()?;
        let width = width.max(1);
        let mut lines = Vec::new();
        if let Some(note) = &pick.note {
            lines.push(Line::from(clip(note, width)));
        }
        let room = settings().rows.max(1);
        for (index, row) in pick.rows.iter().take(room).enumerate() {
            let mark = if index == pick.selected { "> " } else { "  " };
            let mut label = String::from(mark);
            label.push_str(&row.label);
            let style = if index == pick.selected {
                Style::default().add_modifier(Modifier::REVERSED)
            } else {
                Style::default()
            };
            lines.push(Line::from(Span::styled(clip(&label, width), style)));
            for extra in wrapped(&row.detail, width).into_iter().take(2) {
                if !extra.is_empty() {
                    lines.push(Line::styled(extra, self.dim));
                }
            }
        }
        if pick.searching {
            lines.push(Line::styled("searching…", self.dim));
        }
        (!lines.is_empty()).then_some(lines)
    }

    pub fn dismiss(&mut self) -> bool {
        let Some(pick) = self.pick.take() else {
            return false;
        };
        if pick.kind != Kind::Guess {
            self.dismissed = Some(pick.token);
        }
        true
    }

    pub fn move_sel(&mut self, down: bool) -> bool {
        let Some(pick) = self.pick.as_mut() else {
            return false;
        };
        if down && pick.selected + 1 < pick.rows.len().min(settings().rows.max(1)) {
            pick.selected += 1;
        } else if !down {
            pick.selected = pick.selected.saturating_sub(1);
        }
        true
    }

    pub fn tab(&mut self, line: &str, col: usize) -> Act {
        if let Some(pick) = self.pick.take() {
            self.dismissed = None;
            let Some(row) = pick.rows.get(pick.selected) else {
                return Act::Nope;
            };
            return Act::Splice {
                start: pick.start,
                end: pick.end,
                text: row.insert.clone(),
            };
        }
        let ghost = ghost_suffix(line, col);
        if ghost.is_empty() {
            return Act::Nope;
        }
        let mut text = ghost;
        if trailing_space(line, col) {
            text.push(' ');
        }
        self.ghost.clear();
        Act::Insert(text)
    }

    pub fn enter(&mut self, line: &str) -> (Act, bool) {
        let kind = self.pick.as_ref().map(|pick| pick.kind);
        let Some(kind) = kind else {
            return (Act::Nope, false);
        };
        if kind == Kind::Slash && line.split_whitespace().nth(1).is_some() {
            return (Act::Nope, false);
        }
        let act = self.tab(line, 0);
        let submit = kind == Kind::Slash && matches!(act, Act::Splice { .. });
        (act, submit)
    }

    pub fn toggle_guesses(&mut self, line: &str, col: usize) {
        if self
            .pick
            .as_ref()
            .is_some_and(|pick| pick.kind == Kind::Guess)
        {
            self.pick = None;
            return;
        }
        let word = word_before(line, col);
        let guesses = guesses_for(line, col);
        if guesses.is_empty() {
            return;
        }
        let start = col.saturating_sub(word.chars().count());
        self.pick = Some(Pick {
            kind: Kind::Guess,
            rows: guesses
                .into_iter()
                .map(|guess| Row {
                    label: guess.clone(),
                    detail: String::new(),
                    insert: guess,
                })
                .collect(),
            selected: 0,
            start,
            end: col,
            token: word,
            note: None,
            searching: false,
        });
    }

    pub fn known_skill(&mut self, name: &str, cwd: &str) -> bool {
        self.ensure_skills(cwd);
        self.skills
            .as_ref()
            .is_some_and(|(_, skills)| skills.iter().any(|(skill, _)| skill == name))
    }

    fn mark(&mut self, text: &str) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::new();
        for (index, line) in text.split('\n').enumerate() {
            let spans = if self.cache.get(index).is_some_and(|(kept, _)| kept == line) {
                self.cache
                    .get(index)
                    .map(|(_, spans)| spans.clone())
                    .unwrap_or_default()
            } else {
                let spans: Vec<(usize, usize)> = typo_spans(line)
                    .into_iter()
                    .map(|span| (span.start, span.end))
                    .collect();
                if index < self.cache.len() {
                    self.cache[index] = (line.to_owned(), spans.clone());
                } else {
                    self.cache.push((line.to_owned(), spans.clone()));
                }
                spans
            };
            for (start, end) in spans {
                out.push((index, start, end));
            }
        }
        if text.split('\n').count() < self.cache.len() {
            self.cache.truncate(text.split('\n').count());
        }
        out
    }

    fn offer(
        &mut self,
        line: &str,
        _col: usize,
        token: &str,
        cwd: &str,
        session_dir: &str,
    ) -> Option<Pick> {
        let (start, end, kind, query) = classify(line, token)?;
        let limit = settings().rows.max(1);
        let (rows, note, searching) = match kind {
            Kind::Slash => {
                let counts = ledger_for_key(session_dir);
                (slash_rows(&query, cwd, &counts.slashes, self), None, false)
            }
            Kind::File => {
                let (files, capped) = self.file_list(cwd);
                let offer = file_rows(files, Some(files), &query, capped, limit);
                let rows = offer
                    .labels
                    .iter()
                    .map(|path| Row {
                        label: path.clone(),
                        detail: String::new(),
                        insert: format!("{path} "),
                    })
                    .collect();
                (rows, offer.note, offer.searching)
            }
            Kind::Emoji => (emoji_rows(&query), None, false),
            Kind::Scheme => (scheme_rows(&query), None, false),
            Kind::Guess => return None,
        };
        if rows.is_empty() && note.is_none() {
            return None;
        }
        Some(Pick {
            kind,
            rows,
            selected: 0,
            start,
            end,
            token: token.to_owned(),
            note,
            searching,
        })
    }

    fn file_list(&mut self, cwd: &str) -> (&[String], bool) {
        let fresh = self.files.as_ref().is_none_or(|(dir, _, _)| dir != cwd);
        if fresh {
            let mut files = crate::popup::walk_files(Path::new(cwd), PICKER_CAP + 1);
            let more = files.len() > PICKER_CAP;
            files.truncate(PICKER_CAP);
            self.files = Some((cwd.to_owned(), files, more));
        }
        self.files
            .as_ref()
            .map(|(_, files, more)| (files.as_slice(), *more))
            .unwrap_or((&[], false))
    }

    fn ensure_skills(&mut self, cwd: &str) {
        if self.skills.as_ref().is_some_and(|(dir, _)| dir == cwd) {
            return;
        }
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default();
        let found = yi_runtime::skills::discover(Path::new(cwd), &home)
            .into_iter()
            .map(|skill| (skill.name, skill.description))
            .collect();
        self.skills = Some((cwd.to_owned(), found));
    }
}

impl Default for Assist {
    fn default() -> Self {
        Self::new()
    }
}

fn slash_rows(
    query: &str,
    cwd: &str,
    counts: &HashMap<String, u32>,
    assist: &mut Assist,
) -> Vec<Row> {
    assist.ensure_skills(cwd);
    let mut rows = Vec::new();
    for (order, name) in SLASH_COMMANDS.iter().enumerate() {
        if !query.is_empty() && !name.starts_with(query) && !name.contains(query) {
            continue;
        }
        rows.push(ranked_row(
            format!("/{name}"),
            verb_detail(name).to_owned(),
            format!("/{name}"),
            *name == query,
            counts.get(*name).copied().unwrap_or(0),
            order,
        ));
    }
    let skill_names: Vec<(String, String)> = assist
        .skills
        .as_ref()
        .map(|(_, skills)| skills.clone())
        .unwrap_or_default();
    for (order, (name, detail)) in skill_names.into_iter().enumerate() {
        if SLASH_COMMANDS.contains(&name.as_str()) {
            continue;
        }
        if !query.is_empty() && !name.starts_with(query) && !name.contains(query) {
            continue;
        }
        let exact = name == query;
        rows.push(ranked_row(
            format!("/{name}"),
            detail,
            format!("/{name}"),
            exact,
            counts.get(&name).copied().unwrap_or(0),
            SLASH_COMMANDS.len() + order,
        ));
    }
    rows.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)).then(a.2.cmp(&b.2)));
    rows.into_iter().map(|(_, _, _, row)| row).collect()
}

fn ranked_row(
    label: String,
    detail: String,
    insert: String,
    exact: bool,
    count: u32,
    order: usize,
) -> (bool, u32, usize, Row) {
    (
        exact,
        count,
        order,
        Row {
            label,
            detail,
            insert,
        },
    )
}

fn emoji_rows(query: &str) -> Vec<Row> {
    EMOJI
        .iter()
        .filter(|(name, _)| query.is_empty() || name.starts_with(query))
        .map(|(name, emoji)| Row {
            label: format!(":{name}: {emoji}"),
            detail: String::new(),
            insert: (*emoji).to_owned(),
        })
        .collect()
}

fn scheme_rows(query: &str) -> Vec<Row> {
    SCHEMES
        .into_iter()
        .filter(|scheme| {
            let name = scheme
                .chars()
                .take_while(|ch| *ch != ':')
                .collect::<String>();
            query.is_empty() || name.starts_with(query) || scheme.starts_with(query)
        })
        .map(|scheme| Row {
            label: scheme.to_owned(),
            detail: String::new(),
            insert: scheme.to_owned(),
        })
        .collect()
}

fn classify(line: &str, token: &str) -> Option<(usize, usize, Kind, String)> {
    if token.is_empty() {
        return None;
    }
    let (start, end) = token_bounds(line, token);
    if token.starts_with('/') && !token.starts_with("//") && !line.trim_start().starts_with("//") {
        let query: String = token.chars().skip(1).collect();
        if query.contains('/') {
            return None;
        }
        return Some((start, end, Kind::Slash, query));
    }
    if let Some(at) = token.chars().position(|ch| ch == '@') {
        let query: String = token.chars().skip(at + 1).collect();
        let start = start + at;
        return Some((start, end, Kind::File, query));
    }
    if let Some(stripped) = token.strip_prefix(':') {
        if stripped.contains(':') {
            return None;
        }
        return Some((start, end, Kind::Emoji, stripped.to_owned()));
    }
    if scheme_token(token) {
        return Some((start, end, Kind::Scheme, token.to_owned()));
    }
    None
}

fn token_bounds(line: &str, token: &str) -> (usize, usize) {
    let mut start = 0;
    for word in line.split_whitespace() {
        if word == token {
            return (start, start + token.chars().count());
        }
        start += word.chars().count() + 1;
    }
    let count = token.chars().count();
    (
        line.chars().count().saturating_sub(count),
        line.chars().count(),
    )
}

fn token_at(line: &str, col: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let col = col.min(chars.len());
    let mut start = col;
    while start > 0 && !chars[start - 1].is_whitespace() {
        start -= 1;
    }
    let mut end = col;
    while end < chars.len() && !chars[end].is_whitespace() {
        end += 1;
    }
    chars[start..end].iter().collect()
}

fn scheme_token(token: &str) -> bool {
    if token.starts_with(':') || token.starts_with('/') || token.starts_with('@') {
        return false;
    }
    SCHEMES.iter().any(|scheme| {
        let name: String = scheme.chars().take_while(|ch| *ch != ':').collect();
        (token.chars().count() >= 4 && name.starts_with(token))
            || (token.starts_with(&name) && token.contains(':'))
    })
}

fn shortcode(chars: &[char], at: usize) -> Option<(&str, usize)> {
    if chars.get(at) != Some(&':') {
        return None;
    }
    let mut end = at + 1;
    while end < chars.len() && chars[end].is_ascii_alphanumeric() {
        end += 1;
    }
    if chars.get(end) != Some(&':') || end == at + 1 {
        return None;
    }
    let name: String = chars[at + 1..end].iter().collect();
    emoji(&name)?;
    EMOJI
        .iter()
        .find(|(key, _)| *key == name)
        .map(|(key, _)| (*key, end + 1))
}

fn filter_files(names: &[String], query: &str, limit: usize) -> Vec<String> {
    let limit = limit.max(1);
    if query.is_empty() {
        return names.iter().take(limit).cloned().collect();
    }
    let query_l = query.to_lowercase();
    let mut hits: Vec<&String> = names
        .iter()
        .filter(|name| name.to_lowercase().contains(&query_l))
        .collect();
    hits.sort_by(|left, right| {
        let left_exact = file_exact(left, query);
        let right_exact = file_exact(right, query);
        right_exact.cmp(&left_exact).then(left.cmp(right))
    });
    hits.into_iter().take(limit).cloned().collect()
}

fn file_exact(name: &str, query: &str) -> bool {
    name == query || name.rsplit('/').next() == Some(query)
}

fn trailing_space(line: &str, col: usize) -> bool {
    match line.chars().nth(col) {
        None => true,
        Some(ch) if ch.is_whitespace() => false,
        Some(ch) if ".,;:!?)]}'\"".contains(ch) => false,
        Some(_) => true,
    }
}

fn word_before(line: &str, col: usize) -> String {
    let chars: Vec<char> = line.chars().collect();
    let col = col.min(chars.len());
    let mut start = col;
    while start > 0 && chars[start - 1].is_alphabetic() {
        start -= 1;
    }
    chars[start..col].iter().collect()
}

fn clip(text: &str, width: usize) -> String {
    text.chars().take(width.max(1)).collect()
}

fn wrapped(text: &str, width: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let chars: Vec<char> = text.chars().collect();
    let width = width.max(1);
    chars
        .chunks(width)
        .take(2)
        .map(|chunk| chunk.iter().collect())
        .collect()
}

fn verb_detail(name: &str) -> &'static str {
    match name {
        "new" => "start a session",
        "undo" => "restore the last turn's files",
        "quit" => "leave the session",
        "tree" => "open the entry tree",
        "editor" => "edit the draft in an editor",
        "advisor" => "ask the advisor",
        "plan" => "open a plan",
        "plantree" => "show the plan tree",
        "goal" => "set the session goal",
        "agents" => "show the agents",
        "model" => "choose a model",
        "permissions" => "show permission mode",
        "compact" => "compact the session",
        "sessions" => "list sessions",
        "lanes" => "show the lanes",
        "land" => "land the lane",
        "discard" => "discard the lane",
        "heartbeat" => "show the heartbeat",
        "todo" => "show the todo list",
        "theme" => "choose a theme",
        "learn" => "read a skill",
        _ => "",
    }
}

fn emoji(name: &str) -> Option<&'static str> {
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
