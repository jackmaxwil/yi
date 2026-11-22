//! "Always open here" and "Never open by itself" on a pane's bar, in the real window: the
//! word lands in the settings, the row is ticked, and a tab behind speaks for its own pane.

use std::error::Error;

use rune::Runtime;
use yd_core::grid::step::GridMsg;
use yd_core::grid::windows::Scope;
use yd_core::ids::PaneId;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::settings::schema::SettingValue;
use yd_core::settings::store::OPENS;
use yd_ui::win::{Win, dispatch};

use super::{Check, by_key, keys, live, rest, row_text, settled, tap};
use crate::root;

const MENU: &str = "window.context-menu";
const ALWAYS: &str = "window.context-menu.row.pane.always-here";
const NEVER: &str = "window.context-menu.row.pane.never-alone";

/// A right-click on the first tab whose key starts `start` and holds `part`; the key.
pub(super) fn menu(
    runtime: &mut Runtime<Win>,
    start: &str,
    part: &str,
) -> Result<String, Box<dyn Error>> {
    let shown = runtime.elements();
    let tab =
        |key: &&str| key.starts_with(start) && key.contains(part) && key.ends_with(".pane-tab");
    let found = keys(&shown).into_iter().find(tab);
    let key = found
        .ok_or_else(|| format!("no tab {start}…{part}"))?
        .to_owned();
    let [x, y, w, h] = by_key(&shown, &key).ok_or("the tab is gone")?.rect;
    let (x, y) = (x + w / 2.0, y + h / 2.0);
    runtime.pointer_move(x, y);
    runtime.secondary_down(x, y);
    runtime.secondary_up(x, y);
    rest(runtime);
    Ok(key)
}

/// The words on what opens by itself, as the settings hold them.
fn said(runtime: &Runtime<Win>) -> Result<String, Box<dyn Error>> {
    let hub = runtime.state.hub.try_borrow()?;
    match hub.model.settings.value(OPENS) {
        Some(SettingValue::Text(text)) => Ok(text.to_string()),
        other => Err(format!("the words are {other:?}").into()),
    }
}

#[test]
fn a_word_on_a_panes_tab_is_written_ticked_and_taken_back() -> Check {
    let mut runtime = settled("shell/everything")?;
    menu(&mut runtime, "changes-", "")?;
    assert!(live(&runtime, ALWAYS) && live(&runtime, NEVER), "both rows");
    tap(&mut runtime, NEVER)?;
    assert_eq!(said(&runtime)?, "changes never");
    assert!(!live(&runtime, MENU), "the pick closed the menu");
    menu(&mut runtime, "changes-", "")?;
    let shown = runtime.elements();
    assert!(row_text(&shown, NEVER)?.contains('✓'), "the word in force");
    assert!(!row_text(&shown, ALWAYS)?.contains('✓'), "and only it");
    tap(&mut runtime, NEVER)?;
    assert_eq!(said(&runtime)?, "", "picked again, it is taken back");
    // A pane the person opened is the session's until always is said on it.
    runtime.update(|win, _| {
        let (window, kind) = (root::MAIN, PaneKind::Debugger);
        dispatch(win, Msg::Grid(GridMsg::Open { window, kind }));
    });
    rest(&mut runtime);
    let scope = |runtime: &Runtime<Win>| -> Result<Scope, Box<dyn Error>> {
        let hub = runtime.state.hub.try_borrow()?;
        let mut panes = hub.model.grid.panes.iter();
        let found = panes.find(|pane| pane.kind == PaneKind::Debugger);
        Ok(found.ok_or("no Debugger pane")?.scope.clone())
    };
    assert_ne!(scope(&runtime)?, Scope::Window, "the session's own");
    menu(&mut runtime, "debugger-", ".tab-0.")?;
    tap(&mut runtime, ALWAYS)?;
    assert_eq!(said(&runtime)?, "debugger work", "always, where it is");
    assert_eq!(scope(&runtime)?, Scope::Window, "the window's own now");
    Ok(())
}

#[test]
fn the_chat_is_offered_no_word_and_a_tab_behind_speaks_for_its_own_pane() -> Check {
    let mut runtime = settled("shell/everything")?;
    runtime.update(|win, _| {
        let (window, kind) = (root::MAIN, PaneKind::Debugger);
        dispatch(win, Msg::Grid(GridMsg::Open { window, kind }));
    });
    rest(&mut runtime);
    let key = menu(&mut runtime, "", ".tab-pane-")?;
    let id = key
        .rsplit_once(".tab-pane-")
        .and_then(|(_, rest)| rest.split('.').next());
    let behind = PaneId(id.ok_or("no pane in the key")?.parse()?);
    let kind = {
        let hub = runtime.state.hub.try_borrow()?;
        hub.model.grid.kind(behind).ok_or("no such pane")?
    };
    assert!(
        !key.starts_with(kind.slug()),
        "a tab on another pane's bar: {key}"
    );
    let head = row_text(&runtime.elements(), "window.context-menu-head")?;
    let named = head.contains(kind.title()) && head.contains(&format!("yi://pane/{}", behind.0));
    assert!(named, "{head:?} is not {kind:?}");
    tap(&mut runtime, NEVER)?;
    assert_eq!(said(&runtime)?, format!("{} never", kind.slug()));
    menu(&mut runtime, "chat-", "")?;
    assert!(live(&runtime, MENU), "the chat has a menu");
    assert!(!live(&runtime, ALWAYS) && !live(&runtime, NEVER), "no word");
    Ok(())
}
