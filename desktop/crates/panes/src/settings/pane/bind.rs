//! The settings that show state kept elsewhere (the theme with its density and glass, the
//! chat side, the layout): a change to the row changes the state, and the other way round.

use std::borrow::Cow;
use std::io::Write;

use yd_core::command::table::{FLIP_CHAT, Route, Run};
use yd_core::grid::layout::ChatSide;
use yd_core::grid::saved::SAVED;
use yd_core::grid::step::GridMsg;
use yd_core::ids::{SettingKey, ThemeId, WindowId};
use yd_core::msg::{Msg, step};
use yd_core::settings::schema::{Choice, Control, Setting, SettingValue};
use yd_core::settings::store::SettingsMsg;
use yd_ui::grid_view::picker::{CHAT_SIDE_KEY, LAYOUT_KEY};
use yd_ui::pane::Stage;
use yd_ui::theme::files::{self, Library};
use yd_ui::win::{Hub, Win, dispatch};

use super::file;
use crate::themes::editor;
use crate::themes::pane::{DENSITY, GLASS, THEME};

/// The window whose layout the layout setting names.
const MAIN: WindowId = WindowId(0);

fn text(value: impl Into<String>) -> SettingValue {
    SettingValue::Text(Cow::Owned(value.into()))
}

fn side(side: ChatSide) -> &'static str {
    match side {
        ChatSide::Left => "left",
        ChatSide::Right => "right",
    }
}

/// What each bound row should say now; a state no row can say is left out.
fn state(hub: &Hub) -> Vec<(&'static str, SettingValue)> {
    let (tokens, grid) = (&hub.model.tokens, &hub.model.grid);
    let theme = hub
        .panes
        .get::<Library>(None)
        .map(|library| &library.active);
    // The row is how much shows through; the token is how much the tint covers.
    let through = (f64::from(1.0 - tokens.glass_window) * 100.0).round() / 100.0;
    let saved = grid.window(MAIN).and_then(|window| window.saved);
    let rows = [
        (THEME, theme.map(|theme| text(theme.as_str()))),
        (DENSITY, editor::density(tokens).map(text)),
        (GLASS, Some(SettingValue::Number(through))),
        (CHAT_SIDE_KEY, Some(text(side(grid.side)))),
        (LAYOUT_KEY, saved.map(text)),
    ];
    let said = rows.into_iter();
    said.filter_map(|(key, value)| Some((key, value?)))
        .collect()
}

/// The theme row's choices are the themes that can be put in use, files of the user's own
/// included: declared again whenever the library lists others.
pub fn list_themes(hub: &mut Hub) {
    let Some(library) = hub.panes.get::<Library>(None) else {
        return;
    };
    let usable = || library.themes.iter().filter(|t| t.unavailable.is_none());
    let rows = &hub.model.settings.rows;
    let Some(row) = rows.iter().find(|row| row.key.as_str() == THEME) else {
        return;
    };
    let listed = match &row.control {
        Control::Theme(choices) => {
            let ids = choices.iter().map(|choice| choice.value.as_ref());
            ids.eq(usable().map(|theme| theme.id().as_str()))
        }
        _ => true,
    };
    if listed {
        return;
    }
    let choices: Vec<Choice> = usable()
        .map(|theme| Choice {
            label: Cow::Owned(theme.file.name.clone()),
            value: Cow::Owned(theme.id().to_string()),
        })
        .collect();
    let row = Setting {
        control: Control::Theme(Cow::Owned(choices)),
        ..row.clone()
    };
    step(
        &mut hub.model,
        Msg::Settings(SettingsMsg::Declare(vec![row])),
    );
}

/// Makes each bound row say what the state is; the theme in use is kept in the settings
/// file too, so it comes back after a restart. True when a row changed. The poll calls it.
pub fn follow(win: &mut Win) -> bool {
    let Ok(mut hub) = win.hub.try_borrow_mut() else {
        return false;
    };
    // A pane drawn alone shows the rows its scene declared; only the app's window holds
    // the state these rows follow.
    if !matches!(hub.stage, Stage::App) {
        return false;
    }
    list_themes(&mut hub);
    let mut changed = false;
    for (key, value) in state(&hub) {
        let row = hub.model.settings.value(key);
        if row.is_none() || row == Some(&value) {
            continue;
        }
        let key = SettingKey::new(key);
        let set = SettingsMsg::Set(key.clone(), Some(value));
        step(&mut hub.model, Msg::Settings(set));
        if key.as_str() == THEME
            && let Err(why) = file::keep(&mut hub, &key)
        {
            let _ = writeln!(
                std::io::stderr(),
                "yi-desktop: the theme in use was not kept: {why}"
            );
        }
        changed = true;
    }
    changed
}

/// Puts the theme the settings name in use, if the library has it: what start-up does once
/// the settings file and the themes folder are read.
pub fn theme(hub: &mut Hub) {
    let Some(SettingValue::Text(id)) = hub.model.settings.value(THEME).cloned() else {
        return;
    };
    if let Some(library) = files::library(hub) {
        library.pick(&ThemeId::new(id.as_ref()));
    }
    files::settle(hub);
}

/// Makes the state follow the row `key` after a change to it. A value the state cannot
/// take is put right by the next [`follow`].
pub fn apply(win: &mut Win, key: &str) {
    let value = match win.hub.try_borrow() {
        Ok(hub) => hub.model.settings.value(key).cloned(),
        Err(_) => return,
    };
    match (key, value) {
        (THEME, Some(SettingValue::Text(_))) => {
            if let Ok(mut hub) = win.hub.try_borrow_mut() {
                theme(&mut hub);
            }
            files::apply(win, None);
        }
        (DENSITY, Some(SettingValue::Text(density))) => editor::set_density(win, &density),
        (GLASS, Some(SettingValue::Number(through))) => {
            editor::put(win, &[("glass.window", 1.0 - through)]);
        }
        (CHAT_SIDE_KEY, Some(SettingValue::Text(wanted))) => {
            let now = win.hub.try_borrow().map(|hub| side(hub.model.grid.side));
            if now.is_ok_and(|now| now != wanted) {
                dispatch(win, Msg::Command(Route::Model, Run::new(FLIP_CHAT)));
            }
        }
        (LAYOUT_KEY, Some(SettingValue::Text(wanted))) => {
            let saved = SAVED.iter().find(|saved| saved.slug == wanted);
            if let Some(saved) = saved.map(|saved| saved.slug) {
                let window = MAIN;
                dispatch(win, Msg::Grid(GridMsg::Show { window, saved }));
            }
        }
        _ => {}
    }
}
