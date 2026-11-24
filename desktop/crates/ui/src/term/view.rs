//! The terminal view: focus, keys, selection, copy, scrollback and the size the shell is told.

pub mod state;

use std::sync::OnceLock;

use rune::{Drag, El, KeyEvent, Phase, Tx, WheelEvent, col, grid};
use yd_core::feed::SourceCmd;
use yd_core::ids::{FeedId, PaneId};
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;

use crate::pane::{PaneCx, Scene};
use crate::term::keys;
use crate::term::screen::{Cell, Palette, Selection, page};
use crate::theme::look::CODE_LINE;
use crate::win::{Win, dispatch};
use state::{FEED, Shell, Shells, Surface, Tab, lock};

pub const SCENES: &[Scene] = &[];

/// The grid's limits, so a huge pane cannot ask the shell for an absurd size.
const MAX_COLS: f32 = 500.0;
const MAX_ROWS: f32 = 300.0;

/// One mono cell: the grid places cells by it, and the shaped line must match.
pub fn cell() -> (f32, f32) {
    static WIDTH: OnceLock<f32> = OnceLock::new();
    let width =
        WIDTH.get_or_init(|| rune::text::measure(&"0".repeat(100), &[], CODE_LINE, None).0 / 100.0);
    (*width, CODE_LINE.line_px())
}

/// The key of a shell's screen: `<pane key>.screen.<instance>`.
pub fn screen_key(pane: PaneId, feed: FeedId) -> String {
    format!(
        "{}-{}.screen.{}",
        PaneKind::Terminal.slug(),
        pane.0,
        feed.instance
    )
}

/// A shell's screen, filling what it is given; `pane` is the pane that holds it.
pub fn surface(pane: &PaneCx<'_>, shell: &Shell) -> El {
    let palette = Palette::new(pane.tokens());
    let s = &shell.surface;
    let (cw, ch) = cell();
    let (screen, label) = draw(s, &palette);
    let (rows, cols) = screen;
    let (id, feed) = (pane.pane, shell.feed);
    let body = grid(
        cols,
        cw,
        ch,
        s.rows.try_borrow().map(|r| r.rows()).unwrap_or_default(),
    )
    .key(screen_key(id, feed))
    .label(label)
    .w(f32::from(cols) * cw)
    .h(f32::from(rows) * ch)
    .focusable(true)
    .on_key(move |win: &mut Win, ev: &KeyEvent, tx: &mut Tx| key(win, id, feed, ev, tx))
    .on_drag(move |win: &mut Win, d: &mut Drag, tx: &mut Tx| drag(win, id, feed, d, tx))
    .on_wheel(move |win: &mut Win, e: WheelEvent, _: &mut Tx| wheel(win, id, feed, e));
    col()
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(palette.background())
        .px(8.0)
        .py(4.0)
        .child(
            // Incident: unkeyed, one shell's body stood in the place of the one before it, kept
            // its size, and never reported it: the new shell was never told and never started.
            col()
                .key(format!("{}.body", screen_key(id, feed)))
                .w_full()
                .grow(1.0)
                .min_h(0.0)
                .clip_rounded(0.0)
                .on_layout_tx(move |win: &mut Win, r: [f32; 4], _: &mut Tx| fit(win, id, feed, r))
                .child(body),
        )
}

/// Gives the keyboard to the shell `pane` shows, once its screen is drawn: a new or chosen
/// tab is where the person's next keys go. The agent's tab has no screen to type into.
pub fn focus_shown(win: &Win, tx: &mut Tx, pane: PaneId) {
    let active = win.hub.try_borrow().ok().and_then(|hub| {
        let shells = hub.panes.get::<Shells>(Some(pane))?;
        match shells.active {
            Tab::Shell(feed) => Some(feed),
            Tab::Agent => None,
        }
    });
    if let Some(feed) = active {
        tx.focus_when_shown(screen_key(pane, feed), false);
    }
}

/// Rebuilds the rows from the screen or the page scrolled back to: the one place a view reads
/// the parser. The lock is held only while rows are read off it.
fn draw(s: &Surface, palette: &Palette) -> ((u16, u16), String) {
    let parser = lock(&s.parser);
    let size = parser.screen().size();
    let Ok(mut rows) = s.rows.try_borrow_mut() else {
        return (size, String::new());
    };
    if s.back > 0 {
        let paged = page(parser.screen(), &lock(&s.lines), s.back);
        drop(parser);
        rows.refresh(paged.screen(), s.selection, None, CODE_LINE, palette);
        return (size, paged.screen().contents());
    }
    let screen = parser.screen();
    let cursor = (!screen.hide_cursor()).then(|| {
        let (row, col) = screen.cursor_position();
        Cell { row, col }
    });
    rows.refresh(screen, s.selection, cursor, CODE_LINE, palette);
    (size, screen.contents())
}

/// Runs `f` on the surface of `feed` in pane `pane`, with the hub borrowed only inside it.
fn with<R>(
    win: &mut Win,
    pane: PaneId,
    feed: FeedId,
    f: impl FnOnce(&mut Surface) -> R,
) -> Option<R> {
    let mut hub = win.hub.try_borrow_mut().ok()?;
    let shell = hub.panes.get_mut::<Shells>(Some(pane))?.shell_mut(feed)?;
    Some(f(&mut shell.surface))
}

/// Asks something of the shell's source: keys, a size, its end. The app starts the source
/// on the first ask of a feed it is not running.
pub fn send(win: &mut Win, feed: FeedId, ask: SourceCmd) {
    dispatch(win, Msg::Source(feed, ask));
}

fn key(win: &mut Win, pane: PaneId, feed: FeedId, ev: &KeyEvent, tx: &mut Tx) -> bool {
    if ev.is_command('c') {
        let copied = with(win, pane, feed, |s| {
            let sel = s.selection?;
            let parser = lock(&s.parser);
            Some(if s.back > 0 {
                sel.text(page(parser.screen(), &lock(&s.lines), s.back).screen())
            } else {
                sel.text(parser.screen())
            })
        });
        return match copied.flatten() {
            Some(text) => {
                tx.copy(text);
                true
            }
            None => false,
        };
    }
    let base = tx.key_base();
    let bytes = with(win, pane, feed, |s| {
        let app_cursor = lock(&s.parser).screen().application_cursor();
        let bytes = keys::encode(ev, base, app_cursor)?;
        s.back = 0;
        s.selection = None;
        Some(bytes)
    });
    let Some(bytes) = bytes.flatten() else {
        return false;
    };
    send(win, feed, SourceCmd::Input(bytes.into_bytes()));
    true
}

/// Pastes into the terminal that has the keyboard; false when none has it. The app's root
/// `on_paste` calls it, since Rune hands a paste only to the root.
pub fn paste(win: &mut Win, text: &str, tx: &mut Tx) -> bool {
    let Some((pane, feed)) = tx.focused().and_then(parse_key) else {
        return false;
    };
    let bytes = with(win, pane, feed, |s| {
        keys::paste(text, lock(&s.parser).screen().bracketed_paste())
    });
    let Some(bytes) = bytes else {
        return false;
    };
    send(win, feed, SourceCmd::Input(bytes.into_bytes()));
    true
}

fn parse_key(key: &str) -> Option<(PaneId, FeedId)> {
    let rest = key
        .strip_prefix(PaneKind::Terminal.slug())?
        .strip_prefix('-')?;
    let (pane, instance) = rest.split_once(".screen.")?;
    Some((
        PaneId(pane.parse().ok()?),
        FEED.instance(instance.parse().ok()?),
    ))
}

fn cell_at(parser: &vt100::Parser, x: f32, y: f32) -> Cell {
    let (rows, cols) = parser.screen().size();
    let (cw, ch) = cell();
    let at =
        |v: f32, size: f32, n: u16| ((v / size).floor().max(0.0) as u16).min(n.saturating_sub(1));
    Cell {
        row: at(y, ch, rows),
        col: at(x, cw, cols),
    }
}

fn drag(win: &mut Win, pane: PaneId, feed: FeedId, d: &mut Drag, tx: &mut Tx) {
    with(win, pane, feed, |s| {
        let at = cell_at(&lock(&s.parser), d.x, d.y);
        match d.phase {
            Phase::Down => {
                s.anchor = Some(at);
                s.selection = None;
            }
            Phase::Move | Phase::Up => {
                s.selection = s.anchor.map(|anchor| Selection { anchor, head: at })
            }
        }
    });
    if d.phase == Phase::Down {
        tx.focus(screen_key(pane, feed), false);
    }
}

fn wheel(win: &mut Win, pane: PaneId, feed: FeedId, e: WheelEvent) -> bool {
    with(win, pane, feed, |s| {
        let (_, ch) = cell();
        s.wheel += e.dy / ch;
        let lines = s.wheel.trunc();
        s.wheel -= lines;
        let kept = if lock(&s.parser).screen().alternate_screen() {
            0
        } else {
            lock(&s.lines).len()
        };
        let n = lines.abs() as usize;
        let back = if lines > 0.0 {
            s.back.saturating_add(n)
        } else {
            s.back.saturating_sub(n)
        };
        s.back = back.min(kept);
        s.selection = None;
    })
    .is_some()
}

/// The grid follows the pane: whole cells in what layout gave the body, told to the shell.
fn fit(win: &mut Win, pane: PaneId, feed: FeedId, r: [f32; 4]) {
    let (cw, ch) = cell();
    let cols = (r[2] / cw).floor().clamp(1.0, MAX_COLS) as u16;
    let rows = (r[3] / ch).floor().clamp(1.0, MAX_ROWS) as u16;
    let tell = with(win, pane, feed, |s| {
        let mut parser = lock(&s.parser);
        if parser.screen().size() != (rows, cols) {
            parser.set_size(rows, cols);
            s.selection = None;
        }
        (s.told != Some((rows, cols))).then(|| s.told = Some((rows, cols)))
    });
    if tell.flatten().is_some() {
        send(win, feed, SourceCmd::Resize { cols, rows });
    }
}

#[cfg(test)]
mod tests;
