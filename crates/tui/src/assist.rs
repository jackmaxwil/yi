use std::ops::Range;
use std::sync::{Arc, OnceLock};
use std::time::Duration;

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use yi_types::config::{CompleteEngine, TuiConfig};

pub use yi_runtime::spell::Ask;

mod engine;
mod mask;
mod pick;

pub use engine::Words;
use mask::{keep_span, prose_mask};
use pick::{Kind, Pick};

/// Invariant: the error color is the underline, and the letters keep the color they had.
pub fn spell_style(color: Color) -> Style {
    Style::default()
        .underline_color(color)
        .add_modifier(Modifier::UNDERLINED)
}

/// Resolved `tui` assistance: unset keys keep typo marks on, completion on `auto`, autocorrect off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AssistSettings {
    pub typos: bool,
    pub complete: CompleteEngine,
    pub autocorrect: bool,
}

const LIST_ROWS: usize = 10;

impl AssistSettings {
    pub const DEFAULT: Self = Self {
        typos: true,
        complete: CompleteEngine::Auto,
        autocorrect: false,
    };

    pub fn from_tui(tui: Option<&TuiConfig>) -> Self {
        let Some(tui) = tui else {
            return Self::DEFAULT;
        };
        Self {
            typos: tui.typos.unwrap_or(true),
            complete: tui.complete.unwrap_or(CompleteEngine::Auto),
            autocorrect: tui.autocorrect.unwrap_or(false),
        }
    }
}

/// A spelling source the composer asks without waiting: `None` means the answer is still out.
pub trait Engine: Send {
    fn typos(&mut self, line: &str) -> Option<Vec<Range<usize>>>;
    fn words(&mut self, ask: Ask, text: &str) -> Option<Vec<String>>;
    /// A count that moves whenever answers arrive.
    fn poll(&mut self) -> u64 {
        0
    }
    fn waiting(&self) -> bool {
        false
    }
    fn alive(&self) -> bool {
        true
    }
}

/// Where the cursor sits and what the edit was, for [`Assist::sync`].
pub struct At<'a> {
    pub text: &'a str,
    pub row: usize,
    pub col: usize,
    pub cwd: &'a str,
    pub typed: bool,
}

pub enum Act {
    Nope,
    Shown,
    Insert(String),
    Splice {
        start: usize,
        end: usize,
        text: String,
    },
}

pub struct Assist {
    settings: AssistSettings,
    engine: Option<Box<dyn Engine>>,
    words: Arc<OnceLock<Words>>,
    sessions: Option<std::path::PathBuf>,
    text: String,
    marks: Vec<(usize, Range<usize>)>,
    ghost: String,
    pick: Option<Pick>,
    dismissed: Option<String>,
    guess_for: Option<String>,
    seen: u64,
    lists: pick::Sources,
    opened: Option<(usize, usize, Option<char>)>,
    typo: Style,
    dim: Style,
}

impl Assist {
    pub fn new(
        settings: AssistSettings,
        engine: Option<Box<dyn Engine>>,
        words: Arc<OnceLock<Words>>,
    ) -> Self {
        Self {
            settings,
            engine,
            words,
            sessions: None,
            text: String::new(),
            marks: Vec::new(),
            ghost: String::new(),
            pick: None,
            dismissed: None,
            guess_for: None,
            seen: 0,
            lists: pick::Sources::default(),
            opened: None,
            typo: spell_style(Color::Rgb(255, 95, 95)),
            dim: Style::default().add_modifier(Modifier::DIM),
        }
    }

    /// The native engines and the word list from a sessions root. A headless drive passes
    /// `None` and gets lists only, so its frames never depend on the machine.
    pub fn native(native: Option<(AssistSettings, std::path::PathBuf)>) -> Self {
        let Some((settings, sessions)) = native else {
            return Self::new(AssistSettings::DEFAULT, None, Arc::default());
        };
        let mut assist = Self::new(settings, engine::apple(&settings), engine::words());
        assist.sessions = Some(sessions);
        assist
    }

    pub(crate) fn set_styles(&mut self, typo: Style, dim: Style) {
        self.typo = typo;
        self.dim = dim;
    }

    pub(crate) fn typo_style(&self) -> Style {
        self.typo
    }

    pub(crate) fn dim_style(&self) -> Style {
        self.dim
    }

    pub(crate) fn ghost(&self) -> &str {
        &self.ghost
    }

    pub(crate) fn open(&self) -> bool {
        self.pick.is_some()
    }

    pub(crate) fn typo_at(&self, row: usize, col: usize) -> bool {
        (self.marks.iter()).any(|(at, span)| *at == row && span.contains(&col))
    }

    /// How soon the host loop should come back for answers still out.
    pub(crate) fn wake(&self) -> Duration {
        if self.engine.as_ref().is_some_and(|engine| engine.waiting()) {
            Duration::from_millis(10)
        } else {
            Duration::MAX
        }
    }

    /// True when answers arrived since the last sync, so the caller syncs and repaints.
    pub(crate) fn poll(&mut self) -> bool {
        let Some(engine) = self.engine.as_mut() else {
            return false;
        };
        let count = engine.poll();
        let moved = count != self.seen;
        self.seen = count;
        moved || !engine.alive()
    }

    /// True once, when the engine died: spelling is off for the session from here.
    pub(crate) fn lost(&mut self) -> bool {
        let dead = self.engine.as_ref().is_some_and(|engine| !engine.alive());
        if dead {
            self.engine = None;
            self.marks.clear();
            self.guess_for = None;
        }
        dead
    }

    /// Invariant: a list opens only on an edit the user typed. A recalled history entry or a
    /// paste never opens one, so the next arrow still walks history.
    pub fn sync(&mut self, at: &At<'_>) {
        if let Some(sessions) = &self.sessions {
            engine::load_words(&self.words, sessions);
        }
        let line = at.text.split('\n').nth(at.row).unwrap_or_default();
        let edited = at.text != self.text;
        self.text = at.text.to_owned();
        self.marks = self.mark(at.text);
        self.ghost = self.ghost_at(line, at.col);
        self.prefetch(line, at.col);
        let token = pick::token(line, at.col).1;
        if let Some(word) = self.guess_for.clone() {
            if pick::word_before(line, at.col) == word {
                self.offer_guesses(line, at.col);
            } else {
                self.guess_for = None;
            }
        }
        if !edited {
            if self.pick.as_ref().is_some_and(|pick| pick.token != token) {
                self.pick = None;
            }
            return;
        }
        if !at.typed || self.dismissed.as_ref().is_some_and(|held| *held == token) {
            self.pick = None;
            return;
        }
        self.dismissed = None;
        self.pick = self.offer(at, line);
    }

    /// Incident: a file the agent wrote after the first `@` never listed, so each new token
    /// walks the folder again; its later keys, matching or not, narrow over that one walk.
    fn offer(&mut self, at: &At<'_>, line: &str) -> Option<Pick> {
        let (start, token) = pick::token(line, at.col);
        let opened = (at.row, start, token.chars().next());
        if self.opened != Some(opened) {
            self.lists = pick::Sources::default();
            self.opened = Some(opened);
        }
        self.lists.offer(at, line)
    }

    pub(crate) fn lines(&self, width: usize) -> Option<Vec<Line<'static>>> {
        let pick = self.pick.as_ref()?;
        let width = width.max(1);
        let mut lines = Vec::new();
        if let Some(note) = &pick.note {
            lines.push(Line::from(clip(note, width)));
        }
        let room = LIST_ROWS;
        let shown = pick.rows.iter().take(room);
        let pad = shown
            .clone()
            .map(|row| row.label.chars().count())
            .max()
            .unwrap_or(0);
        for (index, row) in shown.enumerate() {
            let (mark, style) = if index == pick.selected {
                (" › ", Style::default().add_modifier(Modifier::REVERSED))
            } else {
                ("   ", Style::default())
            };
            let label = clip(&format!("{mark}{:<pad$}", row.label), width);
            let room = width.saturating_sub(label.chars().count());
            let detail = clip(&format!("  {}", row.detail), room);
            let mut spans = vec![Span::styled(label, style)];
            if !row.detail.is_empty() && room > 2 {
                spans.push(Span::styled(detail, self.dim));
            }
            lines.push(Line::from(spans));
        }
        if let Some(more) = pick.rows.len().checked_sub(room).filter(|more| *more > 0) {
            let note = format!("   … {more} more · list cap {room} · type to narrow");
            lines.push(Line::styled(clip(&note, width), self.dim));
        }
        Some(lines)
    }

    /// The highlighted row's index among the lines [`Assist::lines`] draws.
    pub(crate) fn selected_line(&self) -> usize {
        let pick = self.pick.as_ref();
        pick.map_or(0, |pick| {
            pick.selected
                .saturating_add(usize::from(pick.note.is_some()))
        })
    }

    pub(crate) fn dismiss(&mut self) -> bool {
        self.guess_for = None;
        let Some(pick) = self.pick.take() else {
            return false;
        };
        if pick.kind != Kind::Guess {
            self.dismissed = Some(pick.token);
        }
        true
    }

    pub(crate) fn move_sel(&mut self, down: bool) -> bool {
        let Some(pick) = self.pick.as_mut() else {
            return false;
        };
        if down {
            let last = pick.rows.len().min(LIST_ROWS).saturating_sub(1);
            pick.selected = pick.selected.saturating_add(1).min(last);
        } else {
            pick.selected = pick.selected.saturating_sub(1);
        }
        true
    }

    /// Tab takes what is drawn: the highlighted row, else the ghost. With nothing drawn it
    /// completes the token at the cursor, in place for one match, as a list for several.
    pub(crate) fn tab(&mut self, at: &At<'_>) -> Act {
        if let Some(pick) = self.pick.take() {
            return splice(&pick);
        }
        let line = at.text.split('\n').nth(at.row).unwrap_or_default();
        if !self.ghost.is_empty() {
            let mut text = std::mem::take(&mut self.ghost);
            if pick::space_after(line, at.col) {
                text.push(' ');
            }
            return Act::Insert(text);
        }
        let Some(pick) = self.offer(at, line) else {
            return Act::Nope;
        };
        if pick.rows.len() == 1 {
            return splice(&pick);
        }
        self.pick = Some(pick);
        Act::Shown
    }

    /// Enter on an open list takes its row; a slash row then submits, as the typed name would.
    pub(crate) fn enter(&mut self, line: &str) -> (Act, bool) {
        let Some(kind) = self.pick.as_ref().map(|pick| pick.kind) else {
            return (Act::Nope, false);
        };
        if kind == Kind::Slash && line.split_whitespace().nth(1).is_some() {
            return (Act::Nope, false);
        }
        let act = self.pick.take().map_or(Act::Nope, |pick| splice(&pick));
        (act, kind == Kind::Slash)
    }

    pub(crate) fn toggle_guesses(&mut self, line: &str, col: usize) {
        if self
            .pick
            .as_ref()
            .is_some_and(|pick| pick.kind == Kind::Guess)
        {
            self.pick = None;
            return;
        }
        self.offer_guesses(line, col);
    }

    fn offer_guesses(&mut self, line: &str, col: usize) {
        let word = pick::word_before(line, col);
        self.guess_for = None;
        if word.is_empty() {
            return;
        }
        let Some(engine) = self.engine.as_mut() else {
            return;
        };
        let Some(guesses) = engine.words(Ask::Guesses, &word) else {
            self.guess_for = engine.waiting().then_some(word);
            return;
        };
        self.pick = pick::guesses(guesses, &word, col, pick::token(line, col).1);
    }

    /// The correction for the word before the cursor, as a splice, when one is already known.
    pub(crate) fn correction(&mut self, line: &str, col: usize) -> Option<(usize, usize, String)> {
        let word = pick::word_before(line, col);
        if !self.settings.autocorrect || word.is_empty() {
            return None;
        }
        let fixed = (self
            .engine
            .as_mut()?
            .words(Ask::Correct, &word)?
            .into_iter())
        .next()?;
        let start = col.saturating_sub(word.chars().count());
        (fixed != word).then_some((start, col, fixed))
    }

    fn prefetch(&mut self, line: &str, col: usize) {
        let word = pick::word_before(line, col);
        let wanted = self.settings.autocorrect && !word.is_empty();
        if let Some(engine) = self.engine.as_mut().filter(|_| wanted) {
            let _ = engine.words(Ask::Correct, &word);
        }
    }

    fn mark(&mut self, text: &str) -> Vec<(usize, Range<usize>)> {
        let Some(engine) = self.engine.as_mut().filter(|_| self.settings.typos) else {
            return Vec::new();
        };
        let chars: Vec<char> = text.chars().collect();
        let mask = prose_mask(&chars);
        let mut out = Vec::new();
        let mut offset = 0_usize;
        for (row, line) in text.split('\n').enumerate() {
            let len = line.chars().count();
            let end = offset.saturating_add(len);
            let prose = mask.get(offset..end).is_some_and(|m| m.contains(&true));
            if let Some(spans) = engine.typos(line).filter(|_| prose) {
                let kept = spans.into_iter().filter(|span| {
                    let (from, to) = (
                        offset.saturating_add(span.start),
                        offset.saturating_add(span.end),
                    );
                    keep_span(&chars, &mask, from, to)
                });
                out.extend(kept.map(|span| (row, span)));
            }
            offset = end.saturating_add(1);
        }
        out
    }

    /// Drawn only with the cursor at the line's end, which is the one place the box paints it.
    fn ghost_at(&mut self, line: &str, col: usize) -> String {
        let prefix = pick::word_before(line, col);
        if col != line.chars().count() || prefix.chars().count() < 2 {
            return String::new();
        }
        let ngram = || self.words.get().and_then(|words| words.suffix(&prefix));
        let mut apple = || {
            let word = (self
                .engine
                .as_mut()?
                .words(Ask::Complete, &prefix)?
                .into_iter())
            .next()?;
            pick::suffix_of(&word, &prefix)
        };
        let found = match self.settings.complete {
            CompleteEngine::Off => None,
            CompleteEngine::Ngram => ngram(),
            CompleteEngine::Apple => apple(),
            CompleteEngine::Auto => blend(apple(), ngram()),
        };
        found.unwrap_or_default()
    }
}

impl Default for Assist {
    fn default() -> Self {
        Self::native(None)
    }
}

fn splice(pick: &Pick) -> Act {
    pick.rows
        .get(pick.selected)
        .map_or(Act::Nope, |row| Act::Splice {
            start: pick.start,
            end: pick.end,
            text: row.insert.clone(),
        })
}

/// The longer of two suffixes when one extends the other; otherwise Apple's.
fn blend(apple: Option<String>, ngram: Option<String>) -> Option<String> {
    match (apple, ngram) {
        (Some(apple), Some(ngram)) if ngram.len() > apple.len() && ngram.starts_with(&apple) => {
            Some(ngram)
        }
        (apple, ngram) => apple.or(ngram),
    }
}

/// The emoji for a `:name:` the user just closed at `col`, as a splice of that row. Only a
/// whole token in prose is rewritten, so `std::x::y`, `10:100:` and code keep their colons.
pub fn shortcode_before(
    text: &str,
    row: usize,
    col: usize,
) -> Option<(usize, usize, &'static str)> {
    let line = text.split('\n').nth(row)?;
    let (start, token) = pick::token(line, col);
    let end = start.saturating_add(token.chars().count());
    let name = token.strip_prefix(':')?.strip_suffix(':')?;
    let open_code = line.chars().take(start).filter(|ch| *ch == '`').count() % 2 == 1;
    if end != col || name.is_empty() || name.contains(':') || open_code {
        return None;
    }
    let before = text.split('\n').take(row);
    let offset: usize = before.map(|l| l.chars().count().saturating_add(1)).sum();
    let mask = prose_mask(&text.chars().collect::<Vec<_>>());
    let span = offset.saturating_add(start)..offset.saturating_add(end);
    let prose = mask.get(span).is_some_and(|m| m.iter().all(|p| *p));
    prose
        .then(|| pick::emoji(name))
        .flatten()
        .map(|emoji| (start, end, emoji))
}

pub(crate) fn in_typing_word(line: &str, cursor: usize, cell: usize) -> bool {
    let chars: Vec<char> = line.chars().collect();
    let (start, end) = mask::run(&chars, cursor, |ch| ch.is_alphabetic() || ch == '\'');
    (start..end).contains(&cell)
}

fn clip(text: &str, width: usize) -> String {
    text.chars().take(width.max(1)).collect()
}
