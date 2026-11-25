//! What more than one pane's tests do to a drawn window.
use std::error::Error;

use rune::{Key, KeyEvent, Mods, Runtime};
use yd_ui::win::Win;

/// Presses and lets go at the middle of the live element keyed `key`, then settles as told.
pub(crate) fn tap(
    rt: &mut Runtime<Win>,
    key: &str,
    settle: impl FnOnce(&mut Runtime<Win>),
) -> Result<(), Box<dyn Error>> {
    let all = rt.elements();
    let mut live = all.iter().filter(|e| !e.exiting);
    let hit = live.find(|e| e.key.as_deref() == Some(key));
    let [x, y, w, h] = hit.ok_or_else(|| format!("no element keyed {key}"))?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(x, y);
    rt.pointer_down(x, y);
    rt.pointer_up(x, y);
    settle(rt);
    Ok(())
}

/// Strikes `key`, with the system's own main key held when `cmd`: ⌘ on a Mac, Ctrl elsewhere.
pub(crate) fn press(rt: &mut Runtime<Win>, key: Key, cmd: bool) {
    let mods = Mods {
        cmd,
        logo: cmd && cfg!(target_os = "macos"),
        ctrl: cmd && !cfg!(target_os = "macos"),
        ..Mods::default()
    };
    rt.key(KeyEvent { key, mods });
    rt.frame(0.016);
}
