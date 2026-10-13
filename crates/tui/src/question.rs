//! The card a root's `ask_user` call opens above the composer: one page per question, its text
//! in terminal cells and, on a kitty terminal, its panel, tiles and buttons in one image below.

use std::hash::{Hash, Hasher};

use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use crate::app::{App, Bottom};
use crate::chrome::{Canvas, Rgba, Rounded};
use crate::colors::Theme;
use crate::keymap::{KeyCodeValue, SingleKey};
use crate::orb::kitty;
use crate::popup::{BottomView, PopupResult};

const PREVIEW_LINES: usize = 5;
const SKIPPED: &str = "Skip: go with your recommendation and say which.";
/// Above the logos' ids (7610 on), below the console notebook's 7701. Two ping-pong as the
/// orb's do, so a repaint never shows the ground between delete and place.
const CARD_IDS: [u32; 2] = [7690, 7691];
/// A side preview column needs room for tiles and a readable mock beside them.
const WIDE: usize = 110;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Choice {
    label: String,
    description: Option<String>,
    preview: Option<String>,
}

impl Choice {
    fn from_value(value: &Value) -> Option<Self> {
        let text = |key: &str| {
            let text = value.get(key)?.as_str()?.trim();
            (!text.is_empty()).then(|| text.to_owned())
        };
        let label = match value {
            Value::String(label) => Some(label.trim().to_owned()),
            _ => text("label"),
        }
        .filter(|label| !label.is_empty())?;
        let preview = value
            .get("preview")
            .and_then(Value::as_str)
            .filter(|preview| !preview.trim().is_empty())
            .map(|preview| preview.trim_end().to_owned());
        Some(Self {
            label,
            description: text("description"),
            preview,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Question {
    prompt: String,
    choices: Vec<Choice>,
}

impl Question {
    /// With no option it is an open question: the card is the answer field alone, already typing.
    pub(crate) fn from_args(args: &Value) -> Option<Self> {
        let prompt = args.get("question")?.as_str()?.trim().to_owned();
        let choices: Vec<Choice> = args
            .get("options")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Choice::from_value)
            .collect();
        (!prompt.is_empty()).then_some(Self { prompt, choices })
    }
}

/// The chrome the image paints under one cell rect of the card's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Part {
    Panel,
    Tile { on: bool },
    Badge { on: bool },
    Chip,
    Field { focused: bool },
    Button { primary: bool },
    Preview,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct Piece {
    part: Part,
    col: usize,
    row: usize,
    cols: usize,
    rows: usize,
}

/// One text row built left to right at absolute columns; a span that would overlap the last
/// one, or start past its limit, is dropped rather than drawn over it.
#[derive(Default)]
struct Row {
    spans: Vec<Span<'static>>,
    at: usize,
}

impl Row {
    fn put(&mut self, col: usize, text: &str, style: Style, limit: usize) {
        if col < self.at || col >= limit {
            return;
        }
        let shown = cut(text, limit.saturating_sub(col));
        self.spans
            .push(Span::raw(" ".repeat(col.saturating_sub(self.at))));
        self.at = col.saturating_add(shown.width());
        self.spans.push(Span::styled(shown, style));
    }
}

/// `text` within `room` columns, ending in `…` where it was cut.
fn cut(text: &str, room: usize) -> String {
    if text.width() <= room {
        return text.to_owned();
    }
    let mut kept = String::new();
    let mut used = 0usize;
    for c in text.chars() {
        let wide = c.width().unwrap_or(0);
        if used.saturating_add(wide) >= room {
            break;
        }
        used = used.saturating_add(wide);
        kept.push(c);
    }
    kept.push('…');
    kept
}

fn piece(part: Part, col: usize, row: usize, cols: usize, rows: usize) -> Piece {
    Piece {
        part,
        col,
        row,
        cols,
        rows,
    }
}

pub(crate) struct QuestionView {
    questions: Vec<Question>,
    page: usize,
    cursor: usize,
    typing: bool,
    collapsed: bool,
    pictured: bool,
    other: String,
    answers: Vec<String>,
}

impl QuestionView {
    pub(crate) fn new(questions: Vec<Question>, pictured: bool) -> Self {
        let open = questions
            .first()
            .is_some_and(|question| question.choices.is_empty());
        Self {
            questions,
            page: 0,
            cursor: 0,
            typing: open,
            collapsed: false,
            pictured,
            other: String::new(),
            answers: Vec::new(),
        }
    }

    fn current(&self) -> Option<&Question> {
        self.questions.get(self.page)
    }

    /// The last page's answer closes the card as one message, so the transcript keeps the
    /// user's words as the single record of the pick.
    fn answer(&mut self, text: String) -> PopupResult {
        self.answers.push(text);
        self.page = self.page.saturating_add(1);
        self.cursor = 0;
        self.other.clear();
        if let Some(next) = self.current() {
            self.typing = next.choices.is_empty();
            return PopupResult::Open;
        }
        let reply = match self.answers.as_slice() {
            [only] => only.clone(),
            many => self
                .questions
                .iter()
                .zip(many)
                .map(|(question, answer)| format!("{} → {answer}", question.prompt))
                .collect::<Vec<_>>()
                .join("\n"),
        };
        PopupResult::Insert(reply)
    }

    /// Row `at` past the last option is Other, which takes typing instead of answering.
    fn pick(&mut self, at: usize) -> PopupResult {
        let label = self
            .current()
            .and_then(|question| question.choices.get(at))
            .map(|choice| choice.label.clone());
        match label {
            Some(label) => self.answer(label),
            None => {
                self.cursor = at;
                self.typing = true;
                PopupResult::Open
            }
        }
    }

    /// The card's text rows and the chrome under them, in cells of a `width`-wide bottom block.
    pub(crate) fn card(&self, width: usize, theme: &Theme) -> (Vec<Line<'static>>, Vec<Piece>) {
        let Some(question) = self.current() else {
            return (Vec::new(), Vec::new());
        };
        let (left, right) = (3usize, width.saturating_sub(3));
        let bold = Modifier::BOLD;
        let text = Style::default().fg(theme.text);
        let (mut rows, mut pieces) = (vec![Row::default()], Vec::new());

        let mut head = Row::default();
        let mut col = left;
        if self.questions.len() > 1 {
            let chip = format!(" {}/{} ", self.page.saturating_add(1), self.questions.len());
            pieces.push(piece(Part::Chip, col, rows.len(), chip.width(), 1));
            head.put(
                col,
                &chip,
                Style::default().fg(theme.warning).add_modifier(bold),
                right,
            );
            col = col.saturating_add(chip.width()).saturating_add(1);
        }
        let controls = if self.collapsed {
            "⌃  ✕"
        } else {
            "⌄  ✕"
        };
        let controls_at = right.saturating_sub(controls.width());
        head.put(
            col,
            &question.prompt,
            text.add_modifier(bold),
            controls_at.saturating_sub(2),
        );
        head.put(
            controls_at,
            controls,
            theme.muted_style(),
            right.saturating_add(1),
        );
        rows.push(head);

        if !self.collapsed {
            rows.push(Row::default());
            self.tiles(question, width, theme, &mut rows, &mut pieces);
            rows.push(self.footer(question, left, right, theme, &mut pieces, rows.len()));
            rows.push(Row::default());
        }
        let panel = piece(Part::Panel, 1, 0, width.saturating_sub(2), rows.len());
        pieces.insert(0, panel);
        let lines = rows.into_iter().map(|row| Line::from(row.spans)).collect();
        (lines, pieces)
    }

    fn tiles(
        &self,
        question: &Question,
        width: usize,
        theme: &Theme,
        rows: &mut Vec<Row>,
        pieces: &mut Vec<Piece>,
    ) {
        let (left, right) = (3usize, width.saturating_sub(3));
        let wide = width >= WIDE && question.choices.iter().any(|c| c.preview.is_some());
        let end = match wide {
            true => left.saturating_add(right.saturating_sub(left).saturating_mul(11) / 20),
            false => right,
        };
        let (tile_at, tile_cols) = (
            left.saturating_sub(1),
            end.saturating_sub(left).saturating_add(2),
        );
        let badge = end.saturating_sub(3);
        let text = Style::default().fg(theme.text);
        let first = rows.len();
        for (at, choice) in question.choices.iter().enumerate() {
            let on = at == self.cursor;
            let start = rows.len();
            let mut label = Row::default();
            if on && !self.pictured {
                label.put(left.saturating_sub(2), "▸", theme.accent_style(), left);
            }
            let mut name = choice.label.clone();
            if at == 0 {
                name.push_str(" (Recommended)");
            }
            let style = if on {
                text.add_modifier(Modifier::BOLD)
            } else {
                text
            };
            label.put(left, &name, style, badge.saturating_sub(1));
            let key = at.saturating_add(1).to_string();
            let key_style = if on {
                theme.accent_style()
            } else {
                theme.muted_style()
            };
            label.put(badge.saturating_add(1), &key, key_style, end);
            rows.push(label);
            let mut line = Row::default();
            if let Some(description) = &choice.description {
                line.put(left, description, theme.muted_style(), end);
            }
            rows.push(line);
            if on && !wide {
                rows.extend(inline_preview(choice, left, end, theme));
            }
            pieces.push(piece(Part::Badge { on }, badge, start, 3, 1));
            let height = rows.len().saturating_sub(start);
            pieces.push(piece(Part::Tile { on }, tile_at, start, tile_cols, height));
            rows.push(Row::default());
        }
        let others = question.choices.len();
        let on = self.cursor == others;
        let start = rows.len();
        let mut label = Row::default();
        if on && !self.pictured {
            label.put(left.saturating_sub(2), "▸", theme.accent_style(), left);
        }
        let name = if others == 0 { "Answer" } else { "Other" };
        label.put(
            left,
            name,
            if on {
                text.add_modifier(Modifier::BOLD)
            } else {
                text
            },
            badge,
        );
        let key_style = if on {
            theme.accent_style()
        } else {
            theme.muted_style()
        };
        label.put(
            badge.saturating_add(1),
            &others.saturating_add(1).to_string(),
            key_style,
            end,
        );
        rows.push(label);
        let mut field = Row::default();
        match self.typing || !self.other.is_empty() {
            true => field.put(
                left.saturating_add(1),
                &format!("{}▏", self.other),
                text,
                end,
            ),
            false => field.put(
                left.saturating_add(1),
                "Type your own answer here",
                theme.dim_style(),
                end,
            ),
        }
        rows.push(field);
        pieces.push(piece(Part::Badge { on }, badge, start, 3, 1));
        let field_cols = end.saturating_sub(left);
        pieces.push(piece(
            Part::Field {
                focused: self.typing,
            },
            left,
            start.saturating_add(1),
            field_cols,
            1,
        ));
        pieces.push(piece(Part::Tile { on }, tile_at, start, tile_cols, 2));
        rows.push(Row::default());
        if wide {
            let pane = tile_at.saturating_add(tile_cols).saturating_add(1);
            let span = rows.len().saturating_sub(first).saturating_sub(1);
            pieces.push(piece(
                Part::Preview,
                pane,
                first,
                right.saturating_add(1).saturating_sub(pane),
                span,
            ));
            let chosen = question.choices.get(self.cursor);
            side_preview(
                chosen,
                rows.get_mut(first..).unwrap_or_default(),
                pane.saturating_add(2),
                right,
                theme,
            );
        }
    }

    fn footer(
        &self,
        question: &Question,
        left: usize,
        right: usize,
        theme: &Theme,
        pieces: &mut Vec<Piece>,
        row: usize,
    ) -> Row {
        let last = self.page.saturating_add(1) >= self.questions.len();
        let next = if last { "Submit ⏎" } else { "Next ⏎" };
        let next_at = right.saturating_sub(next.width()).saturating_sub(1);
        let skip_at = next_at.saturating_sub(4).saturating_sub("Skip".width());
        let keys = question.choices.len().saturating_add(1);
        let hint = match self.typing {
            true => "⏎ send · esc back".to_owned(),
            false => format!("1-{keys} choose · ↑↓ move · tab hide · esc close"),
        };
        let mut foot = Row::default();
        foot.put(left, &hint, theme.dim_style(), skip_at.saturating_sub(3));
        foot.put(skip_at, "Skip", theme.muted_style(), next_at);
        foot.put(
            next_at,
            next,
            Style::default().fg(theme.text).add_modifier(Modifier::BOLD),
            right.saturating_add(1),
        );
        pieces.push(piece(
            Part::Button { primary: false },
            skip_at.saturating_sub(1),
            row,
            6,
            1,
        ));
        pieces.push(piece(
            Part::Button { primary: true },
            next_at.saturating_sub(1),
            row,
            next.width().saturating_add(2),
            1,
        ));
        foot
    }
}

fn preview_rows(choice: &Choice, room: usize) -> (Vec<&str>, usize) {
    let all: Vec<&str> = choice
        .preview
        .as_deref()
        .map(|text| text.lines().collect())
        .unwrap_or_default();
    let total = all.len();
    let kept = if total > room {
        room.saturating_sub(1)
    } else {
        total
    };
    (all.into_iter().take(kept).collect(), total)
}

fn inline_preview(choice: &Choice, left: usize, end: usize, theme: &Theme) -> Vec<Row> {
    let (kept, total) = preview_rows(choice, PREVIEW_LINES.saturating_add(1));
    let mut rows: Vec<Row> = kept
        .iter()
        .map(|line| {
            let mut row = Row::default();
            row.put(
                left.saturating_add(1),
                &format!("│ {line}"),
                theme.muted_style(),
                end,
            );
            row
        })
        .collect();
    if total > kept.len() {
        let mut row = Row::default();
        let note = format!(
            "[… showing {} of {total} lines; the card shows at most {PREVIEW_LINES}; ask for the rest in the composer]",
            kept.len()
        );
        row.put(left.saturating_add(1), &note, theme.muted_style(), end);
        rows.push(row);
    }
    rows
}

/// The highlighted option's preview in the side pane: a title row, then as many lines as fit.
fn side_preview(choice: Option<&Choice>, rows: &mut [Row], at: usize, right: usize, theme: &Theme) {
    let Some((title, body)) = rows.split_first_mut() else {
        return;
    };
    title.put(at, "Preview", theme.dim_style(), right);
    let Some(choice) = choice.filter(|choice| choice.preview.is_some()) else {
        if let Some(row) = body.first_mut() {
            row.put(at, "No preview for this option", theme.dim_style(), right);
        }
        return;
    };
    let (kept, total) = preview_rows(choice, body.len());
    for (row, line) in body.iter_mut().zip(&kept) {
        row.put(at, line, Style::default().fg(theme.text), right);
    }
    if total > kept.len()
        && let Some(row) = body.get_mut(kept.len())
    {
        let note = format!(
            "[… {} of {total} lines; pane fits {}; ask for the rest]",
            kept.len(),
            kept.len()
        );
        row.put(at, &note, theme.muted_style(), right);
    }
}

impl BottomView for QuestionView {
    fn lines(&self, width: usize, theme: &Theme) -> Vec<Line<'static>> {
        self.card(width, theme).0
    }

    fn handle_key(&mut self, key: &SingleKey) -> PopupResult {
        let Some(others) = self.current().map(|question| question.choices.len()) else {
            return PopupResult::Close;
        };
        if self.typing {
            return match key.code {
                KeyCodeValue::Enter => {
                    let text = self.other.trim().to_owned();
                    if text.is_empty() {
                        PopupResult::Open
                    } else {
                        self.answer(text)
                    }
                }
                KeyCodeValue::Esc => {
                    self.typing = false;
                    PopupResult::Open
                }
                KeyCodeValue::Backspace => {
                    if self.other.pop().is_none() {
                        self.typing = false;
                    }
                    PopupResult::Open
                }
                KeyCodeValue::Space => {
                    self.other.push(' ');
                    PopupResult::Open
                }
                KeyCodeValue::Char(c) if !key.ctrl && !key.alt => {
                    self.other.push(c);
                    PopupResult::Open
                }
                _ => PopupResult::Open,
            };
        }
        match key.code {
            KeyCodeValue::Esc => PopupResult::Close,
            KeyCodeValue::Tab => {
                self.collapsed = !self.collapsed;
                PopupResult::Open
            }
            KeyCodeValue::Up => {
                self.cursor = self.cursor.saturating_sub(1);
                PopupResult::Open
            }
            KeyCodeValue::Down => {
                self.cursor = self.cursor.saturating_add(1).min(others);
                PopupResult::Open
            }
            KeyCodeValue::Enter => self.pick(self.cursor),
            KeyCodeValue::Char('s') => self.answer(SKIPPED.to_owned()),
            KeyCodeValue::Char(c) => {
                let nth = c
                    .to_digit(10)
                    .and_then(|digit| usize::try_from(digit).ok())
                    .and_then(|digit| digit.checked_sub(1));
                match nth {
                    Some(at) if at <= others => self.pick(at),
                    _ => PopupResult::Open,
                }
            }
            _ => PopupResult::Open,
        }
    }
}

impl App {
    pub(crate) fn note_question(&mut self, tool_name: &str, args: &Value) {
        if tool_name == "ask_user"
            && let Some(question) = Question::from_args(args)
        {
            self.asked.push(question);
        }
    }

    /// Opens at the turn's end: `ask_user` ends the turn, so the card never races a stream.
    pub(crate) fn open_questions(&mut self) {
        let asked = std::mem::take(&mut self.asked);
        if self.bottom.is_none() && !asked.is_empty() {
            self.bottom = Some(Bottom::Question(QuestionView::new(asked, self.kitty)));
            self.scheduler.request();
        }
    }
}

fn rgb(color: ratatui::style::Color, fallback: (u8, u8, u8)) -> (u8, u8, u8) {
    match color {
        ratatui::style::Color::Rgb(r, g, b) => (r, g, b),
        _ => fallback,
    }
}

/// The chrome in pixels for cells of `cell` size: light ink over a dark ground and the reverse,
/// alpha only, so the terminal's own background shows through every surface.
pub(crate) fn paint(
    pieces: &[Piece],
    (cols, rows): (usize, usize),
    cell: (usize, usize),
    theme: &Theme,
) -> Canvas {
    let (cw, ch) = (cell.0 as f32, cell.1 as f32);
    let mut canvas = Canvas::new(cols.saturating_mul(cell.0), rows.saturating_mul(cell.1));
    let ink = |alpha: u8| match theme.dark {
        true => Rgba(255, 255, 255, alpha),
        false => Rgba(0, 0, 0, alpha),
    };
    let well = |alpha: u8| match theme.dark {
        true => Rgba(0, 0, 0, alpha),
        false => Rgba(255, 255, 255, alpha),
    };
    let (ar, ag, ab) = rgb(theme.accent, (0x82, 0xaa, 0xff));
    let (wr, wg, wb) = rgb(theme.warning, (0xff, 0xc7, 0x77));
    let accent = |alpha: u8| Rgba(ar, ag, ab, alpha);
    for piece in pieces {
        let cells = Rounded {
            x: piece.col as f32 * cw,
            y: piece.row as f32 * ch,
            width: piece.cols as f32 * cw,
            height: piece.rows as f32 * ch,
            radius: 0.0,
        };
        let line = (ch / 18.0).max(1.0);
        match piece.part {
            Part::Panel => {
                let mut panel = Rounded {
                    radius: 0.7 * ch,
                    ..cells
                };
                panel.y += 0.3 * ch;
                panel.height -= 0.3 * ch + 0.55 * ch;
                canvas.shadow(panel.shifted(0.14 * ch), 0.42 * ch, Rgba(0, 0, 0, 130));
                canvas.fill(panel, ink(9));
                canvas.stroke(panel, line, ink(30));
            }
            Part::Tile { on } => {
                let tile = Rounded {
                    radius: 0.4 * ch,
                    ..cells.inset(0.0, -0.32 * ch)
                };
                match on {
                    true => {
                        canvas.fill(tile, ink(22));
                        canvas.stroke(tile, line * 1.2, accent(170));
                    }
                    false => {
                        canvas.fill(tile, ink(12));
                        canvas.stroke(tile, line, ink(12));
                    }
                }
            }
            Part::Badge { on } => {
                let side = (0.78 * ch).max(1.6 * cw);
                let badge = Rounded {
                    x: cells.x + cells.width / 2.0 - side / 2.0,
                    y: cells.y + (ch - 0.78 * ch) / 2.0,
                    width: side,
                    height: 0.78 * ch,
                    radius: 0.22 * ch,
                };
                canvas.fill(badge, ink(10));
                canvas.stroke(badge, line, if on { accent(220) } else { ink(55) });
            }
            Part::Chip => {
                let chip = cells.inset(0.0, 0.1 * ch);
                canvas.fill(
                    Rounded {
                        radius: chip.height / 2.0,
                        ..chip
                    },
                    Rgba(wr, wg, wb, 46),
                );
            }
            Part::Field { focused } => {
                let field = Rounded {
                    radius: 0.26 * ch,
                    ..cells.inset(-0.3 * cw, 0.02 * ch)
                };
                canvas.fill(field, well(90));
                canvas.stroke(field, line, if focused { accent(210) } else { ink(34) });
            }
            Part::Button { primary } => {
                let button = Rounded {
                    radius: 0.28 * ch,
                    ..cells.inset(0.0, 0.08 * ch)
                };
                match primary {
                    true => {
                        canvas.fill(button, ink(46));
                        canvas.stroke(button, line, ink(60));
                    }
                    false => canvas.fill(button, ink(16)),
                }
            }
            Part::Preview => {
                let pane = Rounded {
                    radius: 0.4 * ch,
                    ..cells.inset(0.0, -0.32 * ch)
                };
                canvas.fill(pane, well(70));
                canvas.stroke(pane, line, ink(24));
            }
        }
    }
    canvas
}

/// Where the card's image was last placed and what it showed, so an unchanged card re-places
/// nothing and a moved one re-places without repainting.
#[derive(Default)]
pub struct CardTick {
    front: usize,
    shown: bool,
    painted: Option<u64>,
    at: Option<(u16, u16)>,
}

fn cell_pixels() -> Option<(usize, usize)> {
    let size = ratatui::crossterm::terminal::window_size().ok()?;
    if size.columns == 0 || size.rows == 0 || size.width == 0 || size.height == 0 {
        return None;
    }
    Some((
        usize::from(size.width) / usize::from(size.columns),
        usize::from(size.height) / usize::from(size.rows),
    ))
}

pub fn hide(out: &mut impl std::io::Write, state: &mut CardTick) {
    if state.shown {
        for id in CARD_IDS {
            let _ = kitty::delete_id(out, id);
        }
    }
    *state = CardTick::default();
}

pub fn tick(app: &App, out: &mut impl std::io::Write, state: &mut CardTick) {
    match cell_pixels() {
        Some(cell) => tick_with(app, out, state, cell),
        None => hide(out, state),
    }
}

/// Invariant: the image sits at a negative z under the card's own text rows, so the text stays
/// the terminal's glyphs; kitty placements scroll with the text, so a moved card re-places.
pub fn tick_with(
    app: &App,
    out: &mut impl std::io::Write,
    state: &mut CardTick,
    cell: (usize, usize),
) {
    let (Some(Bottom::Question(view)), Some(at), true) = (&app.bottom, app.card_at, app.kitty)
    else {
        return hide(out, state);
    };
    let (lines, pieces) = view.card(app.width, &app.theme);
    let (cols, rows) = (app.width, lines.len());
    let mut hasher = std::hash::DefaultHasher::new();
    (&pieces, cell, cols, rows, app.theme.dark).hash(&mut hasher);
    let key = hasher.finish();
    let (Ok(cols16), Ok(rows16)) = (u16::try_from(cols), u16::try_from(rows)) else {
        return hide(out, state);
    };
    if state.painted == Some(key) {
        if state.at != Some(at) {
            let id = CARD_IDS.get(state.front).copied().unwrap_or(CARD_IDS[0]);
            let _ = kitty::place_under(out, id, at.0, at.1, cols16, rows16);
            state.at = Some(at);
        }
        return;
    }
    let back = 1usize.saturating_sub(state.front);
    let (Some(&back_id), Some(&front_id)) = (CARD_IDS.get(back), CARD_IDS.get(state.front)) else {
        return;
    };
    let canvas = paint(&pieces, (cols, rows), cell, &app.theme);
    let _ = kitty::transmit(out, back_id, &canvas.pixels, (canvas.width, canvas.height));
    let _ = kitty::place_under(out, back_id, at.0, at.1, cols16, rows16);
    if state.shown {
        let _ = kitty::delete_id(out, front_id);
    }
    *state = CardTick {
        front: back,
        shown: true,
        painted: Some(key),
        at: Some(at),
    };
}
