//! What a hand does to the pane: a change to a setting, the group and tab in front, and the
//! row being typed into.

use rune::{Key, KeyEvent, TextEdit};
use yd_core::ids::{PaneId, SettingKey, TabId};
use yd_core::msg::Msg;
use yd_core::settings::schema::{Setting, SettingValue};
use yd_core::settings::store::value::{shown, typed};
use yd_core::settings::store::{self, SettingsMsg};
use yd_ui::win::{Win, dispatch, field as field_of};

use crate::settings::text;

use super::state::{Part, State, Tab};
use super::{bind, file};

/// The pane's state, made on first use.
pub fn with_state(win: &Win, pane: PaneId, change: impl FnOnce(&mut State)) {
    if let Ok(mut hub) = win.hub.try_borrow_mut()
        && let Some(state) = hub.panes.get_mut::<State>(Some(pane))
    {
        change(state);
    }
}

/// One of the pane's text fields as the window holds it.
pub fn field(win: &mut Win, pane: PaneId, part: Part) -> &mut TextEdit {
    win.edits.entry(field_of(pane, part.name())).or_default()
}

/// What `part` holds when the window's field does not have the keyboard.
fn kept(win: &Win, pane: PaneId, part: Part) -> Option<String> {
    let hub = win.hub.try_borrow().ok()?;
    let state = hub.panes.get::<State>(Some(pane));
    match part {
        Part::Search => Some(state.map(|state| state.search.clone()).unwrap_or_default()),
        Part::Text => Some(text::shown(&hub.model, state)),
        Part::Row => None,
    }
}

/// The field a handler edits: the window's own, made to hold what the pane keeps unless it
/// has the keyboard.
pub fn edit(win: &mut Win, pane: PaneId, part: Part) -> &mut TextEdit {
    let words = kept(win, pane, part);
    let own = field(win, pane, part);
    if let Some(words) = words.filter(|words| !own.focused && own.text != *words) {
        *own = TextEdit::new(words);
    }
    own
}

/// Sets `key` (`None` puts the default back) from a control: the text view is written again
/// from the model.
pub fn set(win: &mut Win, pane: PaneId, key: &SettingKey, value: Option<SettingValue>) {
    with_state(win, pane, |state| {
        state.draft = None;
        state.heard = None;
    });
    apply(win, pane, key, value);
}

/// Sets `key` from a line of the text view, which keeps the text as it was typed.
pub fn set_from_text(win: &mut Win, pane: PaneId, key: &SettingKey, value: SettingValue) {
    apply(win, pane, key, Some(value));
}

/// Sets `key`, keeps it in the app's file when it is the app's own, and has the state it
/// shows follow it. Gives what went wrong with the file, if anything.
pub fn put(win: &mut Win, key: &SettingKey, value: Option<SettingValue>) -> Option<String> {
    dispatch(win, Msg::Settings(SettingsMsg::Set(key.clone(), value)));
    let failed = win
        .hub
        .try_borrow_mut()
        .ok()
        .and_then(|mut hub| file::keep(&mut hub, key).err());
    bind::apply(win, key.as_str());
    failed.map(|why| format!("{key} was not saved: {why}", key = key.as_str()))
}

fn apply(win: &mut Win, pane: PaneId, key: &SettingKey, value: Option<SettingValue>) {
    let problem = put(win, key, value);
    with_state(win, pane, |state| state.problem = problem);
}

fn clear_search(win: &mut Win, pane: PaneId) {
    field(win, pane, Part::Search).clear();
    with_state(win, pane, |state| state.search.clear());
}

/// The search words typed so far, kept with the pane.
pub fn searched(win: &mut Win, pane: PaneId) {
    let words = field(win, pane, Part::Search).text.clone();
    with_state(win, pane, |state| state.search = words);
}

/// Escape empties the box; the key is taken only then.
pub fn search_key(win: &mut Win, pane: PaneId, event: &KeyEvent) -> bool {
    let leave = event.key == Key::Escape && !field(win, pane, Part::Search).text.is_empty();
    if leave {
        clear_search(win, pane);
    }
    leave
}

pub fn group(win: &mut Win, pane: PaneId, name: &str) {
    clear_search(win, pane);
    with_state(win, pane, |state| {
        state.group = Some(name.to_owned());
        state.tab = Tab::Group;
    });
}

pub fn tab(win: &mut Win, pane: PaneId, which: TabId) {
    if let Some(tab) = Tab::of(which) {
        clear_search(win, pane);
        with_state(win, pane, |state| state.tab = tab);
    }
}

/// Puts the default back on every setting in `keys` that differs from it.
pub fn reset(win: &mut Win, pane: PaneId, keys: Vec<SettingKey>) {
    keys.iter().for_each(|key| set(win, pane, key, None));
}

/// Opens the row's own field on what it holds now.
pub fn begin(win: &mut Win, pane: PaneId, setting: &Setting) {
    *field(win, pane, Part::Row) = TextEdit::new(shown(setting));
    with_state(win, pane, |state| state.editing = Some(setting.key.clone()));
}

pub fn cancel(win: &mut Win, pane: PaneId) {
    with_state(win, pane, |state| state.editing = None);
}

/// Sets the row from its field; a word its control refuses is said and the field stays open.
pub fn commit(win: &mut Win, pane: PaneId, key: &SettingKey) {
    let words = field(win, pane, Part::Row).text.clone();
    let Ok(hub) = win.hub.try_borrow() else {
        return;
    };
    let row = store::all(&hub.model)
        .into_iter()
        .find(|row| &row.key == key);
    drop(hub);
    let Some(row) = row else {
        return cancel(win, pane);
    };
    match typed(&row, &words) {
        Ok(value) => {
            cancel(win, pane);
            set(win, pane, key, Some(value));
        }
        Err(why) => with_state(win, pane, |state| state.problem = Some(why.to_string())),
    }
}
