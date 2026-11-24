//! A terminal screen as grid rows, with the app's own scrollback: one shaped line per row,
//! non-ASCII cells pinned to their columns, background, selection and cursor as fill runs.

use std::collections::VecDeque;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::rc::Rc;

use rune::{Color, GridRow, Span, TextStyle};
use yd_core::tokens::{Rgba, Tokens, rgb};

use crate::theme::look::colour;
use crate::theme::scale::{TERMINAL_BACKGROUND, TERMINAL_CARET_INK};

/// A cell (row, col) on the drawn screen.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Cell {
    pub row: u16,
    pub col: u16,
}

/// Text selected from `anchor` to `head`, either way round.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Selection {
    pub anchor: Cell,
    pub head: Cell,
}

impl Selection {
    fn ordered(self) -> (Cell, Cell) {
        if self.anchor <= self.head {
            (self.anchor, self.head)
        } else {
            (self.head, self.anchor)
        }
    }

    /// The columns `start..end` of `row` the selection covers.
    fn on(self, row: u16, cols: u16) -> Option<(u16, u16)> {
        let (a, b) = self.ordered();
        if row < a.row || row > b.row {
            return None;
        }
        let start = if row == a.row { a.col } else { 0 };
        let end = if row == b.row {
            b.col.saturating_add(1).min(cols)
        } else {
            cols
        };
        (end > start).then_some((start, end))
    }

    /// The selected text as the screen holds it (wrapped rows join, others end in a newline).
    pub fn text(self, screen: &vt100::Screen) -> String {
        let (_, cols) = screen.size();
        let (a, b) = self.ordered();
        screen.contents_between(
            a.row,
            a.col.min(cols),
            b.row,
            b.col.saturating_add(1).min(cols),
        )
    }
}

/// The colours a screen is drawn in, read from the tokens.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Palette {
    fg: Color,
    bg: Color,
    cursor: Color,
    selection: Color,
    caret_ink: Color,
    ansi: [Color; 16],
}

/// The xterm colours the shared tokens do not name, each its own `terminal.ansi.<n>` token.
const ANSI: [(u8, u8, u8); 16] = [
    (0x1d, 0x1f, 0x24),
    (0xf5, 0x85, 0x85),
    (0x7f, 0xcf, 0x9a),
    (0xe3, 0xc0, 0x6d),
    (0x8f, 0xb4, 0xff),
    (0xc6, 0x9b, 0xf0),
    (0x78, 0xd2, 0xc6),
    (0xb3, 0xb7, 0xc2),
    (0x5c, 0x60, 0x6b),
    (0xff, 0xa0, 0xa0),
    (0x9a, 0xe6, 0xb0),
    (0xf0, 0xd0, 0x8a),
    (0xa8, 0xc6, 0xff),
    (0xd8, 0xb4, 0xf8),
    (0x9a, 0xe4, 0xd8),
    (0xdc, 0xdf, 0xe6),
];

impl Palette {
    pub fn new(tokens: &Tokens) -> Self {
        let shared = |n: usize| -> Option<Rgba> {
            match n {
                1 => Some(tokens.bad),
                2 => Some(tokens.ok),
                4 => Some(tokens.accent),
                6 => Some(tokens.need),
                7 => Some(tokens.dim),
                15 => Some(tokens.fg),
                _ => None,
            }
        };
        let ansi = std::array::from_fn(|n| {
            let (r, g, b) = ANSI.get(n).copied().unwrap_or_default();
            let named = || tokens.colour_or(&format!("terminal.ansi.{n}"), rgb(r, g, b));
            colour(shared(n).unwrap_or_else(named))
        });
        Self {
            fg: colour(tokens.dim),
            bg: colour(TERMINAL_BACKGROUND.of(tokens)),
            cursor: colour(tokens.fg),
            selection: colour(tokens.selected),
            caret_ink: colour(TERMINAL_CARET_INK.of(tokens)),
            ansi,
        }
    }

    /// The body's fill.
    pub fn background(&self) -> Color {
        self.bg
    }

    fn key(&self) -> u64 {
        let mut h = DefaultHasher::new();
        for c in std::iter::once(&self.fg).chain(&self.ansi).chain([
            &self.cursor,
            &self.selection,
            &self.caret_ink,
        ]) {
            [c.r, c.g, c.b, c.a].map(f32::to_bits).hash(&mut h);
        }
        h.finish()
    }

    /// xterm's 256 colours: 16 named, a 6x6x6 cube, 24 greys.
    pub(crate) fn rgb(&self, c: vt100::Color) -> Option<Color> {
        let level = |v: u8| {
            if v == 0 {
                0
            } else {
                v.saturating_mul(40).saturating_add(55)
            }
        };
        match c {
            vt100::Color::Default => None,
            vt100::Color::Rgb(r, g, b) => Some(linear((r, g, b))),
            vt100::Color::Idx(i @ 0..=15) => self.ansi.get(usize::from(i)).copied(),
            vt100::Color::Idx(i @ 16..=231) => {
                let i = i.saturating_sub(16);
                Some(linear((level(i / 36), level((i / 6) % 6), level(i % 6))))
            }
            vt100::Color::Idx(i) => {
                let v = i.saturating_sub(232).saturating_mul(10).saturating_add(8);
                Some(linear((v, v, v)))
            }
        }
    }
}

fn srgb(v: u8) -> f32 {
    let c = f32::from(v) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear((r, g, b): (u8, u8, u8)) -> Color {
    Color {
        r: srgb(r),
        g: srgb(g),
        b: srgb(b),
        a: 1.0,
    }
}

/// The screen `back` lines up into the scrollback: kept lines above the top of the live
/// screen, laid out on a parser of the screen's size so it draws and copies the same way.
pub fn page(screen: &vt100::Screen, lines: &VecDeque<Vec<u8>>, back: usize) -> vt100::Parser {
    let (rows, cols) = screen.size();
    let mut page = vt100::Parser::new(rows, cols, 0);
    let back = back.min(lines.len());
    let kept = lines.iter().skip(lines.len().saturating_sub(back));
    // Invariant: a width past the screen keeps a wrap from carrying across rows (as the source).
    let live = screen.rows_formatted(0, u16::MAX);
    for (row, bytes) in (1..=rows).zip(kept.cloned().chain(live)) {
        page.process(format!("\x1b[{row};1H\x1b[0m").as_bytes());
        page.process(&bytes);
    }
    page
}

/// The rows last handed to the grid, each with the hash of what built it.
#[derive(Default)]
pub struct Rows {
    cache: Vec<(u64, Rc<GridRow>)>,
    /// Rows rebuilt by the last [`Rows::refresh`].
    pub rebuilt: usize,
}

impl Rows {
    pub fn rows(&self) -> Vec<Rc<GridRow>> {
        self.cache.iter().map(|(_, r)| Rc::clone(r)).collect()
    }

    pub fn refresh(
        &mut self,
        screen: &vt100::Screen,
        selection: Option<Selection>,
        cursor: Option<Cell>,
        style: TextStyle,
        palette: &Palette,
    ) {
        let (rows, cols) = screen.size();
        self.cache.truncate(usize::from(rows));
        self.rebuilt = 0;
        let look = palette.key();
        for row in 0..rows {
            let sel = selection.and_then(|s| s.on(row, cols));
            let caret = cursor.filter(|c| c.row == row).map(|c| c.col);
            let key = row_hash(screen, row, cols, (sel, caret, look));
            let slot = usize::from(row);
            if self.cache.get(slot).is_some_and(|(k, _)| *k == key) {
                continue;
            }
            let built = (
                key,
                Rc::new(build(screen, row, (sel, caret), style, palette)),
            );
            match self.cache.get_mut(slot) {
                Some(old) => *old = built,
                None => self.cache.push(built),
            }
            self.rebuilt = self.rebuilt.saturating_add(1);
        }
    }
}

type Marks = (Option<(u16, u16)>, Option<u16>);

fn row_hash(
    screen: &vt100::Screen,
    row: u16,
    cols: u16,
    marks: (Option<(u16, u16)>, Option<u16>, u64),
) -> u64 {
    let mut h = DefaultHasher::new();
    (cols, marks, screen.row_wrapped(row)).hash(&mut h);
    for col in 0..cols {
        if let Some(c) = screen.cell(row, col) {
            (c.contents(), c.is_wide(), look(c)).hash(&mut h);
        }
    }
    h.finish()
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Look {
    fg: vt100::Color,
    bg: vt100::Color,
    bold: bool,
    italic: bool,
    underline: bool,
    inverse: bool,
}

impl Hash for Look {
    fn hash<H: Hasher>(&self, h: &mut H) {
        let key = |c: vt100::Color| match c {
            vt100::Color::Default => (0, 0, 0, 0),
            vt100::Color::Idx(i) => (1, i, 0, 0),
            vt100::Color::Rgb(r, g, b) => (2, r, g, b),
        };
        let flags = (self.bold, self.italic, self.underline, self.inverse);
        (key(self.fg), key(self.bg), flags).hash(h);
    }
}

fn look(c: &vt100::Cell) -> Look {
    Look {
        fg: c.fgcolor(),
        bg: c.bgcolor(),
        bold: c.bold(),
        italic: c.italic(),
        underline: c.underline(),
        inverse: c.inverse(),
    }
}

/// One row's line text with its spans, background runs and pinned cells.
fn build(
    screen: &vt100::Screen,
    row: u16,
    (sel, caret): Marks,
    style: TextStyle,
    palette: &Palette,
) -> GridRow {
    let (_, cols) = screen.size();
    let (mut text, mut spans, mut fills, mut pinned) =
        (String::new(), Vec::new(), Vec::new(), Vec::new());
    let mut run: Option<(usize, Color, Look)> = None;
    let mut fill: Option<(u16, Color)> = None;
    let close = |spans: &mut Vec<Span>, run: Option<(usize, Color, Look)>, end: usize| {
        if let Some((start, ink, l)) = run.filter(|(start, _, _)| end > *start) {
            let mut s = Span::new(start, end).color(ink);
            if l.bold {
                s = s.bold();
            }
            if l.italic {
                s = s.italic();
            }
            if l.underline {
                s = s.underline(ink);
            }
            spans.push(s);
        }
    };
    for col in 0..cols {
        let Some(cell) = screen.cell(row, col) else {
            break;
        };
        if cell.is_wide_continuation() {
            continue;
        }
        let l = look(cell);
        let (fg, bg) = (palette.rgb(l.fg).unwrap_or(palette.fg), palette.rgb(l.bg));
        let (mut ink, bg) = if l.inverse {
            (bg.unwrap_or(palette.bg), Some(fg))
        } else {
            (fg, bg)
        };
        if caret == Some(col) {
            ink = palette.caret_ink;
        }
        let start = text.len();
        if run.is_none_or(|(_, i, rl)| i != ink || rl != l) {
            close(&mut spans, run, start);
            run = Some((start, ink, l));
        }
        if fill.map(|(_, c)| c) != bg {
            if let Some((from, c)) = fill.take() {
                fills.push((from, col, c));
            }
            fill = bg.map(|c| (col, c));
        }
        let contents = cell.contents();
        // Invariant: a glyph from a fallback font has its own advance and would push every later
        // cell off its column, so any non-ASCII cell is drawn pinned (rune grid.rs docs).
        if !contents.is_ascii() {
            text.push_str(if cell.is_wide() { "  " } else { " " });
            pinned.push((col, if cell.is_wide() { 2 } else { 1 }, contents, ink));
        } else if contents.is_empty() {
            text.push(' ');
        } else {
            text.push_str(&contents);
        }
    }
    close(&mut spans, run, text.len());
    if let Some((from, c)) = fill {
        fills.push((from, cols, c));
    }
    let mut out = GridRow::new(text, spans, style).wraps(screen.row_wrapped(row));
    for (start, end, c) in fills {
        out = out.fill(start, end, c);
    }
    if let Some((start, end)) = sel {
        out = out.fill(start, end, palette.selection);
    }
    if let Some(col) = caret {
        out = out.fill(col, col.saturating_add(1), palette.cursor);
    }
    for (col, width, contents, ink) in pinned {
        out = out.cell(col, width, contents, style, ink);
    }
    out
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;
    use std::error::Error;
    use std::rc::Rc;

    use yd_core::tokens::Tokens;

    use super::{Cell, Palette, Rows, Selection, page};
    use crate::theme::look::CODE_LINE;

    fn screen(bytes: &[u8]) -> vt100::Parser {
        let mut p = vt100::Parser::new(4, 12, 0);
        p.process(bytes);
        p
    }

    /// Dies with a screen that redraws every row each frame: a 200,000-line `yes` would
    /// reshape every row, not only the ones that moved.
    #[test]
    fn an_unchanged_row_keeps_its_row() -> Result<(), Box<dyn Error>> {
        let mut p = screen(b"one\r\ntwo\r\n");
        let (mut rows, palette) = (Rows::default(), Palette::new(&Tokens::default()));
        rows.refresh(p.screen(), None, None, CODE_LINE, &palette);
        let first = rows.rows();
        assert_eq!(rows.rebuilt, 4);
        p.process(b"three");
        rows.refresh(p.screen(), None, None, CODE_LINE, &palette);
        assert_eq!(rows.rebuilt, 1, "only the row written changed");
        let again = rows.rows();
        assert!(Rc::ptr_eq(
            first.first().ok_or("row 0")?,
            again.first().ok_or("row 0")?
        ));
        Ok(())
    }

    /// Dies with a copy that drops a wide character or joins two lines.
    #[test]
    fn a_selection_copies_what_the_screen_shows() {
        let p = screen("日本 ok\r\nnext line".as_bytes());
        let sel = Selection {
            anchor: Cell { row: 1, col: 3 },
            head: Cell { row: 0, col: 0 },
        };
        assert_eq!(sel.text(p.screen()), "日本 ok\nnext");
    }

    /// Dies with a scrollback page that shows the wrong lines, drops their colour, or panics
    /// past one screen as `vt100` 0.15 does.
    #[test]
    fn a_page_back_shows_kept_lines_above_the_screen() -> Result<(), Box<dyn Error>> {
        let p = screen(b"live 1\r\nlive 2");
        let lines: VecDeque<Vec<u8>> = (1..=10)
            .map(|n| format!("\x1b[31mold {n}\x1b[m").into_bytes())
            .collect();
        let back = page(p.screen(), &lines, 3);
        assert_eq!(back.screen().contents(), "old 8\nold 9\nold 10\nlive 1");
        let red = back.screen().cell(0, 0).ok_or("no cell")?.fgcolor();
        assert_eq!(red, vt100::Color::Idx(1));
        let far = page(p.screen(), &lines, 50);
        assert_eq!(far.screen().contents(), "old 1\nold 2\nold 3\nold 4");
        Ok(())
    }
}
