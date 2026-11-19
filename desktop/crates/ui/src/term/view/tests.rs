use std::cell::RefCell;
use std::error::Error;
use std::rc::Rc;

use rune::{Cx, El, Env, Key, KeyEvent, Mods, Runtime, col};
use yd_core::feed::SourceCmd;
use yd_core::ids::{PaneId, WindowId};
use yd_core::msg::Effect;

use super::state::{Shells, lock};
use super::{cell, screen_key, surface};
use crate::pane::PaneCx;
use crate::win::{Hub, Win};

const PANE: PaneId = PaneId(7);
const PAD: (f32, f32) = (8.0, 4.0);

type Sent = Rc<RefCell<Vec<SourceCmd>>>;

fn view(win: &Win, _: &mut Cx<Win>) -> El {
    let Ok(hub) = win.hub.try_borrow() else {
        return col();
    };
    let pane = PaneCx {
        hub: &hub,
        pane: PANE,
        focused: true,
        edits: None,
    };
    match pane.state::<Shells>().and_then(|s| s.tabs.first()) {
        Some(shell) => surface(&pane, shell),
        None => col(),
    }
}

/// A surface in a window of `size`, its screen fed `bytes`, and every command it sends.
fn runtime(size: (f32, f32), bytes: &[u8]) -> Result<(Runtime<Win>, Sent), Box<dyn Error>> {
    let shared = Hub::new(&[], Vec::new());
    let sent: Sent = Rc::default();
    {
        let mut hub = shared.try_borrow_mut()?;
        let log = Rc::clone(&sent);
        hub.set_sink(move |effect| {
            if let Effect::Source(_, ask) = effect {
                log.borrow_mut().push(ask);
            }
        });
        let shells = hub.panes.get_mut::<Shells>(Some(PANE)).ok_or("no shells")?;
        let shell = shells.tabs.first().ok_or("no shell")?;
        lock(&shell.surface.parser).process(bytes);
    }
    let mut rt = Runtime::new(
        Win::new(shared, WindowId(1)),
        view,
        size,
        Env::new(true, 1.0),
    );
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    Ok((rt, sent))
}

fn with_shells<R>(rt: &Runtime<Win>, f: impl FnOnce(&Shells) -> R) -> Result<R, Box<dyn Error>> {
    let hub = rt.state.hub.try_borrow()?;
    Ok(f(hub.panes.get::<Shells>(Some(PANE)).ok_or("no shells")?))
}

fn feed_key(rt: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let feed = with_shells(rt, |s| s.tabs.first().map(|shell| shell.feed))?.ok_or("no shell")?;
    Ok(screen_key(PANE, feed))
}

fn shown(rt: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let key = feed_key(rt)?;
    let screen = rt
        .elements()
        .into_iter()
        .find(|e| e.key.as_deref() == Some(key.as_str()));
    Ok(screen.and_then(|e| e.label).ok_or("no screen")?)
}

/// Dies with keys that never leave the window: typed text and Ctrl-C must reach the shell's
/// source as the bytes an xterm sends.
#[test]
fn typed_keys_go_to_the_shell() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = runtime((600.0, 400.0), b"")?;
    let key = feed_key(&rt)?;
    rt.update(|_, tx| tx.focus(key, false));
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent {
        key: Key::Text("a".into()),
        mods: Mods::default(),
    });
    rt.key(KeyEvent {
        key: Key::Text("c".into()),
        mods: Mods {
            ctrl: true,
            ..Mods::default()
        },
    });
    rt.frame(1.0 / 60.0);
    let sent = sent.borrow();
    let inputs = sent.iter().filter_map(|ask| match ask {
        SourceCmd::Input(bytes) => Some(bytes.as_slice()),
        _ => None,
    });
    assert_eq!(inputs.collect::<Vec<_>>(), [b"a", b"\x03"]);
    Ok(())
}

/// Dies with a grid that keeps its first size: the shell must hear the pane's size in whole
/// cells, and again when the pane shrinks.
#[test]
fn the_grid_follows_the_pane_and_tells_the_shell() -> Result<(), Box<dyn Error>> {
    let (mut rt, sent) = runtime((600.0, 400.0), b"")?;
    let (cw, ch) = cell();
    let want = |w: f32, h: f32| {
        let cols = ((w - 2.0 * PAD.0) / cw).floor() as u16;
        let rows = ((h - 2.0 * PAD.1) / ch).floor() as u16;
        SourceCmd::Resize { cols, rows }
    };
    rt.set_viewport((300.0, 200.0), 1.0);
    for _ in 0..3 {
        rt.frame(1.0 / 60.0);
    }
    let resizes: Vec<SourceCmd> = sent
        .borrow()
        .iter()
        .filter(|ask| matches!(ask, SourceCmd::Resize { .. }))
        .cloned()
        .collect();
    assert_eq!(resizes, [want(600.0, 400.0), want(300.0, 200.0)]);
    let size = with_shells(&rt, |s| {
        s.tabs
            .first()
            .map(|shell| lock(&shell.surface.parser).screen().size())
    })?;
    let rows = ((200.0 - 2.0 * PAD.1) / ch).floor() as u16;
    let cols = ((300.0 - 2.0 * PAD.0) / cw).floor() as u16;
    assert_eq!(size, Some((rows, cols)));
    Ok(())
}

/// Dies with a drag that selects the wrong cells, or a copy with nothing in it.
#[test]
fn a_drag_selects_and_copy_takes_it() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = runtime((600.0, 400.0), b"first line\r\nsecond line\r\n")?;
    let clipboard = rune::clipboard::memory();
    rt.use_clipboard(&clipboard);
    let (cw, ch) = cell();
    let at = |col: f32, row: f32| (PAD.0 + col * cw + cw / 2.0, PAD.1 + row * ch + ch / 2.0);
    let (x, y) = at(6.0, 0.0);
    rt.pointer_down(x, y);
    let (x, y) = at(5.0, 1.0);
    rt.pointer_move(x, y);
    rt.pointer_up(x, y);
    rt.frame(1.0 / 60.0);
    rt.key(KeyEvent {
        key: Key::Command('c'),
        mods: Mods {
            cmd: true,
            logo: true,
            ..Mods::default()
        },
    });
    rt.frame(1.0 / 60.0);
    let copied = clipboard.text().map(|t| t.to_string());
    assert_eq!(copied.as_deref(), Some("line\nsecond"));
    Ok(())
}

/// Dies with scrollback that stops at one screen: the wheel must reach lines the parser
/// itself no longer holds, and a key must bring the live screen back.
#[test]
fn the_wheel_reaches_kept_lines_and_a_key_comes_back() -> Result<(), Box<dyn Error>> {
    let (mut rt, _) = runtime((600.0, 120.0), b"live\r\n")?;
    with_shells(&rt, |s| {
        if let Some(shell) = s.tabs.first() {
            lock(&shell.surface.lines).extend((1..=500).map(|n| format!("old {n}").into_bytes()));
        }
    })?;
    rt.wheel_xy(100.0, 50.0, 0.0, 20_000.0);
    rt.frame(1.0 / 60.0);
    let top = shown(&rt)?;
    assert_eq!(
        top.lines().next(),
        Some("old 1"),
        "the oldest kept line, far past one screen"
    );
    let key = feed_key(&rt)?;
    rt.update(|_, tx| tx.focus(key, false));
    rt.key(KeyEvent {
        key: Key::Text("x".into()),
        mods: Mods::default(),
    });
    rt.frame(1.0 / 60.0);
    assert_eq!(shown(&rt)?.lines().next(), Some("live"));
    Ok(())
}
