//! The code view: lines on `virtual_rows`, one rich text a line, and slots as rows of their
//! own between lines. Built from data every run of a pane's view; it keeps nothing itself.

use std::borrow::Cow;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;
use std::sync::Arc;

use rune::{
    Align, Color, Cursor, El, GrowthSpec, LineCap, PointerEvent, ScrollSpec, SelectionChange,
    Shape, Span, TextHighlight, TextSelection, TextStyle, Tx, col, el, memo, rich, row, text,
};
use rune_kit::syntax::{Syntax, SyntaxTheme};
use yd_core::row::Tone;
use yd_core::tokens::Tokens;

use super::colour::Pending;
use super::doc::{Cols, Doc, LineNo};
use super::line::ink::{dots, wave};
use super::line::{Advance, Put, Shown, word_at};
use super::marks::{Glyph, GutterMark, LineMarks, Marks, Tint};
use super::pointer::{CodeEvent, Token};
use super::rows::{Lines, Row, line_key, line_of, row_at, row_count};
use super::state::{self, Hear, Pointer};
use crate::pane::Scene;
use crate::theme::look::{CODE, CODE_LINE, colour};
use crate::theme::scale::{BREAKPOINT, CODE_INLAY, CODE_VALUE};
use crate::win::{Hub, Win};

#[cfg(test)]
pub(crate) mod sample;
#[cfg(test)]
mod tests;

pub const SCENES: &[Scene] = &[];

/// The gutter's columns (record 4.3, 6.2): marks, line number, change sign. A number keeps
/// eight points clear of the sign column until it has four digits.
const MARK_W: f32 = 13.0;
const NUMBER_W: f32 = 28.0;
const NUMBER_PAD: f32 = 8.0;
const SIGN_W: f32 = 11.0;
/// An inline hint's face: a size under the code's (record 6.4.2).
const HINT_SIZE: f32 = 9.5;
const HINT: TextStyle = TextStyle::new(HINT_SIZE, 400.0).family(CODE);

/// Which columns stand left of the line numbers and right of them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Gutter {
    /// A column for breakpoints, as a debugger's source has.
    pub marks: bool,
    /// A column for the `+` and `-` of changed lines.
    pub signs: bool,
}

impl Default for Gutter {
    fn default() -> Gutter {
        Gutter {
            marks: false,
            signs: true,
        }
    }
}

/// The surface's colours, from the theme's tokens.
#[derive(Debug, Clone, Copy, PartialEq, Hash)]
pub struct Look {
    pub fg: Color,
    pub dim: Color,
    pub faint: Color,
    pub accent: Color,
    pub selection: Color,
    pub need: Color,
    pub ok: Color,
    pub bad: Color,
    pub warn: Color,
    pub model: Color,
    pub context: Color,
    pub breakpoint: Color,
    pub value: Color,
    pub inlay: Color,
    pub syntax: SyntaxTheme,
}

impl Look {
    /// The theme's colours with Rune's seventeen kinds mapped onto its five code colours.
    pub fn of(tokens: &Tokens) -> Look {
        let fg = colour(tokens.fg);
        let plain = [
            Syntax::Function,
            Syntax::Property,
            Syntax::Punctuation,
            Syntax::Parameter,
            Syntax::Operator,
            Syntax::Variable,
            Syntax::Attribute,
        ];
        let syntax = plain
            .into_iter()
            .fold(SyntaxTheme::neutral(true), |theme, kind| {
                theme.with(kind, fg)
            });
        let syntax = syntax
            .with(Syntax::Keyword, colour(tokens.keyword))
            .with(Syntax::Type, colour(tokens.type_name))
            .with(Syntax::String, colour(tokens.string))
            .with(Syntax::Number, colour(tokens.number))
            .with(Syntax::Comment, colour(tokens.comment));
        Look {
            fg,
            dim: colour(tokens.dim),
            faint: colour(tokens.faint),
            accent: colour(tokens.accent),
            selection: crate::a11y::selection(tokens),
            need: colour(tokens.need),
            ok: colour(tokens.ok),
            bad: colour(tokens.bad),
            warn: colour(tokens.warn),
            model: colour(tokens.model),
            context: colour(tokens.context),
            breakpoint: colour(BREAKPOINT.of(tokens)),
            value: colour(CODE_VALUE.of(tokens)),
            inlay: colour(CODE_INLAY.of(tokens)),
            syntax,
        }
    }

    fn tone(&self, tone: Tone) -> Color {
        match tone {
            Tone::Fg => self.fg,
            Tone::Dim => self.dim,
            Tone::Faint => self.faint,
            Tone::Accent => self.accent,
            Tone::Need => self.need,
            Tone::Ok => self.ok,
            Tone::Bad => self.bad,
            Tone::Warn => self.warn,
            Tone::Model => self.model,
            Tone::Context => self.context,
        }
    }
}

/// Where things sit across a line, measured once for each run of the view.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Geometry {
    adv: Advance,
    marks_w: f32,
    /// The width of the widest line number.
    digits_w: f32,
    /// Where a line's text starts across its row.
    code_x: f32,
    line_h: f32,
}

impl Geometry {
    fn of(doc: &Doc, gutter: Gutter, line_h: f32) -> Geometry {
        let cell = |style: TextStyle| rune::text::advance("0000000000000000", style) / 16.0;
        let adv = Advance {
            code: cell(CODE_LINE),
            hint: cell(HINT),
        };
        let digits = doc.lines().checked_ilog10().unwrap_or(0).saturating_add(1);
        let digits = f32::from(u8::try_from(digits).unwrap_or(u8::MAX));
        let digits_w = (NUMBER_W - NUMBER_PAD).max((digits * adv.code).ceil());
        let marks_w = if gutter.marks { MARK_W } else { 0.0 };
        let sign_w = if gutter.signs { SIGN_W } else { 0.0 };
        Geometry {
            adv,
            marks_w,
            digits_w,
            code_x: marks_w + NUMBER_W.max(digits_w + 2.0) + sign_w,
            line_h,
        }
    }

    /// Where the lines under code sit below a row's top: two points under the baseline.
    fn under(&self) -> f32 {
        self.line_h - 2.0
    }

    fn bits(&self) -> [u32; 5] {
        [
            self.adv.code,
            self.adv.hint,
            self.digits_w,
            self.code_x,
            self.line_h,
        ]
        .map(f32::to_bits)
    }
}

/// Who hears which lines the viewport holds.
type Seen = Rc<dyn Fn(&mut Win, Lines)>;

struct Slot {
    name: String,
    build: Rc<dyn Fn(&Hub) -> El>,
}

/// One text on the code surface, keyed `key`: its lines are `<key>.L<n>`.
pub struct CodeView {
    key: String,
    doc: Arc<Doc>,
    colours: Pending,
    marks: Rc<Marks>,
    /// The slots in line order, and beside each the 0-based line it sits under.
    slots: Vec<Slot>,
    after: Vec<usize>,
    hear: Hear,
    seen: Seen,
    look: Look,
    gutter: Gutter,
    geo: Geometry,
    /// Moves when the look or the measures do, so kept rows are drawn again.
    stamp: u64,
}

impl CodeView {
    /// A surface for `doc` whose keys start with `key`, the pane's key or one under it.
    pub fn new(key: impl Into<String>, tokens: &Tokens, doc: Arc<Doc>, colours: Pending) -> Self {
        let gutter = Gutter::default();
        CodeView {
            key: key.into(),
            geo: Geometry::of(&doc, gutter, tokens.line),
            doc,
            colours,
            marks: Rc::default(),
            slots: Vec::new(),
            after: Vec::new(),
            hear: Rc::new(|_, _, _| {}),
            seen: Rc::new(|_, _| {}),
            look: Look::of(tokens),
            gutter,
            stamp: 0,
        }
    }

    /// What the caller puts on the lines.
    pub fn marks(mut self, marks: Rc<Marks>) -> CodeView {
        self.marks = marks;
        self
    }

    pub fn gutter(mut self, gutter: Gutter) -> CodeView {
        self.geo = Geometry::of(&self.doc, gutter, self.geo.line_h);
        self.gutter = gutter;
        self
    }

    /// Hears the surface: hovers, presses and the selection.
    pub fn on_event(mut self, f: impl Fn(&mut Win, CodeEvent, &mut Tx) + 'static) -> CodeView {
        self.hear = Rc::new(f);
        self
    }

    /// Hears which lines the viewport holds, when they change. It runs no view.
    pub fn on_visible(mut self, f: impl Fn(&mut Win, Lines) + 'static) -> CodeView {
        self.seen = Rc::new(f);
        self
    }

    /// Puts what `build` returns between `after` and the line under it, as wide as the surface
    /// and as tall as it lays out; the lines below move down. Its row is `<key>.slot.<name>`.
    pub fn slot(
        mut self,
        after: LineNo,
        name: impl Into<String>,
        build: impl Fn(&Hub) -> El + 'static,
    ) -> CodeView {
        let at = self.after.partition_point(|line| *line <= after.index());
        self.after.insert(at, after.index());
        self.slots.insert(
            at,
            Slot {
                name: name.into(),
                build: Rc::new(build),
            },
        );
        self
    }

    /// The scroll viewport, keyed `<key>.code`, over the lines.
    pub fn build(mut self) -> El {
        let lines = self.doc.lines();
        let mut stamp = DefaultHasher::new();
        (self.look, self.gutter, self.geo.bits()).hash(&mut stamp);
        self.stamp = stamp.finish();
        let mut version = DefaultHasher::new();
        (self.doc.rev(), lines, &self.after).hash(&mut version);
        self.slots
            .iter()
            .for_each(|slot| slot.name.hash(&mut version));
        let wide = self.doc.widest().saturating_add(self.marks.widest());
        let width = self.geo.code_x + cells(wide.saturating_add(2)) * self.geo.adv.code;
        let count = row_count(lines, self.slots.len());
        let view = Rc::new(self);
        let (keys, rows, copy, heard, seen) = (
            view.clone(),
            view.clone(),
            view.clone(),
            view.clone(),
            view.clone(),
        );
        let list = col()
            .key(format!("{}.code.lines", view.key))
            .w_full()
            .min_w(width)
            .growth(GrowthSpec::default())
            .selection_group(true)
            .selection_copy(move |selection| copied(&copy.doc, &copy.marks, selection))
            .on_selection_change(
                move |win: &mut Win, change: &SelectionChange, tx: &mut Tx| {
                    (heard.hear)(win, CodeEvent::Selection(selected(change)), tx);
                },
            )
            .virtual_rows(
                count,
                view.geo.line_h,
                move |_: &Win, i| keys.row_key(i),
                move |win: &Win, i| rows.row(win, i),
            )
            .virtual_rows_version(version.finish())
            .on_visible_rows(move |win: &mut Win, range| {
                let lines = range.filter_map(|i| match seen.row_at(i) {
                    Some(Row::Line(line)) => Some(LineNo::from_index(line)),
                    _ => None,
                });
                let (first, last) = lines.fold((None, None), |(first, _), line| {
                    (first.or(Some(line)), Some(line))
                });
                if let (Some(first), Some(last)) = (first, last) {
                    (seen.seen)(win, Lines::new(first, last));
                }
                false
            });
        col()
            .key(format!("{}.code", view.key))
            .w_full()
            .grow(1.0)
            .min_h(0.0)
            .scroll(ScrollSpec {
                stick_bottom: false,
            })
            .scroll_xy()
            .child(col().w_full().child(list))
    }

    fn row_at(&self, i: usize) -> Option<Row> {
        row_at(self.doc.lines(), &self.after, i)
    }

    /// A line's row is keyed by its number and a slot's by its name, under this view's key:
    /// a memo's id is one name in the whole window.
    fn row_key(&self, i: usize) -> Cow<'static, str> {
        match self.row_at(i) {
            Some(Row::Line(line)) => Cow::Owned(line_key(&self.key, LineNo::from_index(line))),
            Some(Row::Slot(slot)) => {
                let name = self.slots.get(slot).map_or("", |slot| &slot.name);
                Cow::Owned([&self.key, ".slot.", name].concat())
            }
            None => Cow::Borrowed(""),
        }
    }

    fn row(self: &Rc<Self>, win: &Win, i: usize) -> El {
        let hub = win.hub.try_borrow().ok();
        match self.row_at(i) {
            Some(Row::Line(line)) => {
                let no = LineNo::from_index(line);
                let hub = hub.as_deref();
                let hovered = hub
                    .and_then(|hub| state::hovered(hub, &self.key))
                    .filter(|token| token.line == no)
                    .map(|token| token.cols);
                let hand = hub.and_then(|hub| state::hand(hub, &self.key)) == Some(no);
                let version = (
                    self.doc.rev(),
                    self.marks.line(no),
                    self.marks.tint_of(no),
                    hovered,
                    hand,
                    self.colours.get().is_some(),
                    self.stamp,
                );
                memo(line_key(&self.key, no), version, || {
                    self.line(no, hovered, hand)
                })
            }
            Some(Row::Slot(slot)) => self
                .slots
                .get(slot)
                .zip(hub)
                .map_or_else(el, |(slot, hub)| (slot.build)(&hub)),
            None => el(),
        }
    }

    /// `line` as it is drawn, with its hints in it.
    fn shown(&self, line: LineNo) -> Shown {
        shown(&self.doc, &self.marks, line)
    }

    /// A line's row: its number and its text, which fills the row so a drag can start past its
    /// end, with whatever marks the line has laid over them. A plain line is three elements.
    fn line(self: &Rc<Self>, no: LineNo, hovered: Option<Cols>, hand: bool) -> El {
        let (l, g) = (&self.look, &self.geo);
        let key = line_key(&self.key, no);
        let none = LineMarks::default();
        let marks = self.marks.line(no).unwrap_or(&none);
        let face = CODE_LINE.lh(g.line_h);
        let number = text(no.get().to_string())
            .font(face)
            .color(l.faint)
            .align(Align::End);
        let number = number
            .w(g.digits_w)
            .ml(g.marks_w)
            .mr(g.code_x - g.marks_w - g.digits_w);
        let shown = self.shown(no);
        let end = shown.x_of(shown.text().len(), g.adv, |at| measured(&shown, at));
        let mut line = row()
            .w_full()
            .h(g.line_h)
            .child(number)
            .child(self.code(no, &shown, marks, hovered, hand));
        if let Some(value) = &marks.value {
            let label = text(value.as_str())
                .font(HINT.lh(13.0).italic())
                .color(l.value);
            line = line.child(
                row()
                    .key(format!("{key}.value"))
                    .absolute(g.code_x + end + 14.0, (g.line_h - 13.0) / 2.0)
                    .h(13.0)
                    .px(5.0)
                    .radius(3.0)
                    .fill(l.accent.alpha(0.1))
                    .child(label),
            );
        }
        let tint = match self.marks.tint_of(no) {
            Some(Tint::Added) => Some((l.ok, 0.0, Some("+"))),
            Some(Tint::Removed) => Some((l.bad, 0.12, Some("-"))),
            Some(Tint::Current) => Some((l.need, 0.16, None)),
            None => None,
        };
        if let Some((ink, alpha, sign)) = tint {
            let bar = el()
                .key(format!("{key}.bar"))
                .absolute(0.0, 0.0)
                .size(2.0, g.line_h)
                .fill(ink);
            line = line.fill(ink.alpha(alpha)).child(bar);
            let sign = sign.filter(|_| self.gutter.signs).map(|sign| {
                text(sign)
                    .key(format!("{key}.sign"))
                    .font(face)
                    .color(l.faint)
                    .absolute(g.code_x - SIGN_W, 0.0)
            });
            line = line.children(sign);
        }
        line = match marks.gutter {
            Some(GutterMark::Breakpoint) => line.child(
                el().key(format!("{key}.breakpoint"))
                    .absolute(3.0, (g.line_h - 7.0) / 2.0)
                    .size(7.0, 7.0)
                    .radius(3.5)
                    .fill(l.breakpoint),
            ),
            None => line,
        };
        line = match marks.glyph {
            Some(Glyph::Fix) => line.child(
                el().key(format!("{key}.fix"))
                    .absolute(2.0, 3.0)
                    .size(8.0, 8.0)
                    .radius(4.0)
                    .fill(l.need),
            ),
            None => line,
        };
        let (moved, left, pressed, asked) =
            (self.clone(), self.clone(), self.clone(), self.clone());
        line.on_pointer_move_local(move |win: &mut Win, p: PointerEvent, tx: &mut Tx| {
            let to = moved.pointer(no, p.x);
            state::point(win, &moved.key, Some(to), &moved.hear, tx);
        })
        .on_hover_change(move |win: &mut Win, inside, tx: &mut Tx| {
            if inside {
                tx.live_only();
            } else {
                state::leave(win, &left.key, no, &left.hear, tx);
            }
        })
        .on_background_press_tx(move |win: &mut Win, tx: &mut Tx| pressed.press(win, no, tx))
        .on_secondary_press_local(move |win: &mut Win, p: PointerEvent, tx: &mut Tx| {
            let event = CodeEvent::SecondaryPress {
                line: no,
                token: asked.pointer(no, p.x).token,
                at: [p.window_x, p.window_y],
            };
            (asked.hear)(win, event, tx);
            // The line takes the press, so the window's own right-click never hears it.
            crate::menu::context::press(win, p, tx);
        })
    }

    /// A line's text, coloured, with its hints in it and its squiggles and underlines under it.
    fn code(
        &self,
        no: LineNo,
        shown: &Shown,
        marks: &LineMarks,
        hovered: Option<Cols>,
        hand: bool,
    ) -> El {
        let (l, g) = (&self.look, &self.geo);
        let source = self.doc.line(no).unwrap_or("");
        let place = |cols: Cols| shown.drawn(source, cols.within(source)?);
        let x = |at: usize| shown.x_of(at, g.adv, |at| measured(shown, at));
        let lit = l.context.alpha(0.2);

        let toks = self.colours.get().map_or(&[][..], |c| c.line(no)).iter();
        let mut spans: Vec<Span> = toks
            .filter_map(|tok| {
                place(tok.cols).map(|r| Span::new(r.start, r.end).color(l.syntax.color(tok.kind)))
            })
            .collect();
        for (put, _) in shown.puts().iter().filter(|(_, kind)| *kind == Put::Hint) {
            spans.push(Span::new(put.start, put.end).size(HINT_SIZE).color(l.faint));
            spans.push(
                Span::new(put.start.saturating_add(1), put.end.saturating_sub(1)).bg(l.inlay, 3.0),
            );
        }

        let mut under = Shape::new();
        let mut highlights: Vec<TextHighlight> = hovered
            .and_then(place)
            .map(|r| TextHighlight::new(r, lit).pad(0.0))
            .into_iter()
            .collect();
        for (cols, tone) in &marks.squiggles {
            let Some(r) = place(*cols) else { continue };
            under = under.stroke(wave(x(r.start), x(r.end), g.under()), 1.0, l.tone(*tone));
        }
        for (cols, open) in &marks.follows {
            let Some(r) = place(*cols) else { continue };
            let (x0, x1, y) = (x(r.start), x(r.end), g.under());
            if *open {
                highlights.push(TextHighlight::new(r, lit).color(l.fg).pad(0.0));
                under = under.line([x0, y], [x1, y], 1.0, LineCap::Butt, l.accent);
            } else {
                let dot = |x: f32| ([x, y], [x + 1.0, y]);
                under = dots(x0, x1).map(dot).fold(under, |shape, (a, b)| {
                    shape.line(a, b, 1.0, LineCap::Butt, l.context.alpha(0.55))
                });
            }
        }

        let mut code = rich(shown.text(), spans)
            .font(CODE_LINE.lh(g.line_h))
            .color(l.fg)
            .truncate_fade_width(0.0)
            .grow(1.0)
            .key([&self.key, ".row.L", &no.get().to_string()].concat());
        code = code
            .selectable(true)
            .selection_color(l.selection)
            .span_radius(2.0)
            .text_highlights(highlights)
            .shape(under);
        code = shown
            .puts()
            .iter()
            .fold(code, |t, (put, _)| t.uncopied(put.clone()));
        if hand {
            code.cursor(Cursor::Pointer)
        } else {
            code
        }
    }

    /// Where the pointer is, `x` points along the row of `line`.
    fn pointer(&self, line: LineNo, x: f32) -> Pointer {
        let gutter = x < self.geo.code_x;
        Pointer {
            line,
            token: (!gutter)
                .then(|| self.token_at(line, x - self.geo.code_x))
                .flatten(),
            gutter,
        }
    }

    /// A press where the pointer was last seen on `line`.
    fn press(&self, win: &mut Win, line: LineNo, tx: &mut Tx) {
        let seen = win
            .hub
            .try_borrow()
            .ok()
            .and_then(|hub| state::pointer(&hub, &self.key))
            .filter(|pointer| pointer.line == line);
        let event = match seen {
            Some(Pointer { gutter: true, .. }) => Some(CodeEvent::GutterPress(line)),
            Some(Pointer { token, .. }) if tx.mods().cmd => token.map(CodeEvent::CommandPress),
            Some(Pointer { token, .. }) => token.map(CodeEvent::Press),
            None => None,
        };
        if let Some(event) = event {
            (self.hear)(win, event, tx);
        }
    }

    /// The token `x` points along the text of `line`: a name to follow, else the word there.
    fn token_at(&self, line: LineNo, x: f32) -> Option<Token> {
        let source = self.doc.line(line)?;
        let shown = self.shown(line);
        let at = shown.at_x(x, self.geo.adv, |at| measured(&shown, at))?;
        let col = shown.to_source(at)?;
        let follows = self
            .marks
            .line(line)
            .map_or(&[][..], |marks| marks.follows.as_slice());
        if let Some((cols, _)) = follows
            .iter()
            .find(|(cols, _)| u32::try_from(col).is_ok_and(|col| cols.contains(col)))
        {
            return Some(Token {
                line,
                cols: *cols,
                follows: true,
            });
        }
        let word = word_at(source, col)?;
        let cols = Cols::new(
            u32::try_from(word.start).ok()?,
            u32::try_from(word.end).ok()?,
        );
        Some(Token {
            line,
            cols,
            follows: false,
        })
    }
}

impl std::fmt::Debug for CodeView {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeView")
            .field("key", &self.key)
            .field("slots", &self.after)
            .finish_non_exhaustive()
    }
}

/// Brings `line` of the surface keyed `key` to the top of its view, built or not.
pub fn reveal(tx: &mut Tx, key: &str, line: LineNo) {
    tx.scroll_key_to(line_key(key, line), 0.0);
}

/// A count of cells as a float; a text of more than 65,535 cells scrolls that wide.
fn cells(n: usize) -> f32 {
    f32::from(u16::try_from(n).unwrap_or(u16::MAX))
}

/// `line` of `doc` as it is drawn with the hints `marks` put in it.
fn shown(doc: &Doc, marks: &Marks, line: LineNo) -> Shown {
    let hints = marks
        .line(line)
        .map_or(&[][..], |marks| marks.hints.as_slice());
    Shown::new(doc.line(line).unwrap_or(""), hints)
}

/// How far along a line drawn byte `at` starts, from the shaper: for a line whose characters
/// are not all one cell wide.
fn measured(shown: &Shown, at: usize) -> f32 {
    let Some(before) = shown.text().get(..at) else {
        return 0.0;
    };
    let hints = shown
        .puts()
        .iter()
        .filter(|(put, kind)| *kind == Put::Hint && put.start < at);
    let sizes: Vec<Span> = hints
        .map(|(put, _)| Span::new(put.start, put.end.min(at)).size(HINT_SIZE))
        .collect();
    rune::text::measure(before, &sizes, CODE_LINE, None).0
}

/// The line a selected text belongs to and the bytes of it that are selected, as drawn.
fn selected_line(text: &rune::SelectedText) -> Option<(LineNo, std::ops::Range<usize>)> {
    Some((line_of(text.key()?)?, text.range()))
}

/// The lines a selection touches.
fn selected(change: &SelectionChange) -> Option<Lines> {
    let texts = change.selection()?.texts().iter();
    let mut lines = texts
        .filter(|text| text.is_selected())
        .filter_map(selected_line);
    let (first, _) = lines.next()?;
    let last = lines.next_back().map_or(first, |(line, _)| line);
    Some(Lines::new(first, last))
}

/// What a selection copies: the source between its two ends, so a hint is left out and a tab
/// comes back as a tab. Nothing when an end is not in a line of code (a slot's own text).
fn copied(doc: &Doc, marks: &Marks, selection: &TextSelection) -> Option<String> {
    let end = |text: &rune::SelectedText| {
        let (line, range) = selected_line(text)?;
        let drawn = shown(doc, marks, line);
        let from = doc.span(line.index(), line.index()).start;
        Some((
            from.saturating_add(drawn.source_before(range.start)),
            from.saturating_add(drawn.source_before(range.end)),
        ))
    };
    let mut selected = selection.texts().iter().filter(|text| text.is_selected());
    let first = selected.next()?;
    let last = selected.next_back().unwrap_or(first);
    doc.text()
        .get(end(first)?.0..end(last)?.1)
        .map(str::to_owned)
}
