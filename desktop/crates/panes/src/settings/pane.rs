//! Settings pane: groups, setting rows, search, and the same settings as text.

pub mod act;
pub mod bind;
mod controls;
pub mod fields;
pub mod file;
mod groups;
mod rows;
mod scenes;
pub mod state;
#[cfg(test)]
mod tests;

use rune::{Cx, El, Icon, Tx, col, el, row};
use yd_core::address::thing::Thing;
use yd_core::address::uri::Address;
use yd_core::command::by_kind::{SETTING_COPY, SETTING_RESET};
use yd_core::command::table::{Route, Run, Target};
use yd_core::ids::{CommandId, PaneId, SettingKey};
use yd_core::kind::Kind;
use yd_core::model::Model;
use yd_core::msg::Msg;
use yd_core::pane::PaneKind;
use yd_core::reduce::state::Link;
use yd_core::reduce::step::commands::SETTINGS_READ;
use yd_core::settings::store;
use yd_ui::chrome::tabs::{FILE, SLIDERS, Tab, Tabs};
use yd_ui::grid_view::picker;
use yd_ui::line_icon;
use yd_ui::menu::command_box;
use yd_ui::overlay::pick::Finder;
use yd_ui::pane::{PaneCx, PaneDef, TabAsk};
use yd_ui::theme::look::colour;
use yd_ui::win::{Hub, Win, dispatch};

use self::state::{Part, State, Tab as Which};
use crate::settings::text;
use yd_ui::states::status::bar;

use super::states::WORDS;
use crate::copy::empty::{SERVER, down};

static CHECK: Icon = line_icon!("check", r#"<path d="M5 12.5l4.5 4.5L19 7"/>"#);

pub const DEF: PaneDef = PaneDef {
    kind: PaneKind::Settings,
    view,
    scenes: scenes::SCENES,
    commands: &[],
    settings: &[],
    tabs: Some(tabs),
    on_tab: Some(on_tab),
    finder: Some(Finder::Keyed(thing_at)),
};

/// A row's setting: `<pane key>.row.<setting key>`.
fn thing_at(hub: &Hub, _: PaneId, key: &str) -> Option<Thing> {
    let (_, setting) = key.split_once(".row.")?;
    Some(Thing::at(
        &hub.model,
        Address::new(Kind::Setting, setting).ok()?,
    ))
}

/// Carries out a setting's right-click rows: its key to the clipboard, its default back.
pub fn carry(win: &mut Win, tx: &mut Tx, command: CommandId, target: &Target) -> bool {
    let Target::Thing(setting) = target else {
        return false;
    };
    if setting.kind() != Kind::Setting {
        return false;
    }
    match command {
        SETTING_COPY => tx.copy(setting.id().to_owned()),
        SETTING_RESET => {
            act::put(win, &SettingKey::new(setting.id()), None);
        }
        _ => return false,
    }
    true
}

/// The groups a reader meets first, in this order; the rest follow as they were declared.
const FIRST: [&str; 3] = ["Appearance", "Layout", "Motion"];

/// Every setting with each group's rows together. Incident: in declaration order the list
/// opened on a pane's own setting and split Appearance in two round the server's.
pub fn listed(model: &yd_core::model::Model) -> Vec<yd_core::settings::schema::Setting> {
    let mut rows = store::all(model);
    let mut seen: Vec<String> = FIRST.iter().map(|name| (*name).to_owned()).collect();
    for row in &rows {
        if !seen.iter().any(|name| *name == row.group) {
            seen.push(row.group.to_string());
        }
    }
    rows.sort_by_key(|row| seen.iter().position(|name| *name == row.group));
    rows
}

/// The chosen group, every setting, and the ones that differ, each counted from the rows.
fn tabs(pane: &PaneCx<'_>) -> Tabs {
    let rows = listed(pane.model());
    let fresh = State::default();
    let state = pane.state::<State>().unwrap_or(&fresh);
    let count = |n: usize| u32::try_from(n).ok();
    let tab = |which: Which, icon: &'static Icon, title: String, count: Option<u32>| Tab {
        id: which.id(),
        icon,
        title,
        count,
        meta: String::new(),
        // The cross on All and Changed goes back to the group's tab, which has none.
        fixed: which == Which::Group,
    };
    let group = state
        .group_in(&rows)
        .unwrap_or_else(|| PaneKind::Settings.title().to_owned());
    Tabs {
        tabs: vec![
            tab(Which::Group, &SLIDERS, group, None),
            tab(Which::All, &FILE, "All".to_owned(), count(rows.len())),
            tab(
                Which::Changed,
                &CHECK,
                "Changed".to_owned(),
                count(store::browse::changed(&rows)),
            ),
        ],
        active: Some(state.tab.id()),
        adds: false,
        tool: None,
    }
}

/// A press on a tab brings it in front. The cross the bar draws on a tab sends it back to
/// the group; the group's own tab stays, and there is no `+`.
fn on_tab(win: &mut Win, pane: PaneId, ask: TabAsk) {
    match ask {
        TabAsk::Show(tab) => act::tab(win, pane, tab),
        TabAsk::Close(tab) if tab != Which::Group.id() => act::tab(win, pane, Which::Group.id()),
        TabAsk::Close(_) | TabAsk::New => {}
    }
}

/// Asks Yi for its rows once a link is up and none came: zero size, keyed by when it came up.
fn read_yi(model: &Model, key: &str) -> Option<El> {
    let Link::Up { since, .. } = &model.wire.link else {
        return None;
    };
    let read = |win: &mut Win, _: [f32; 4], _: &mut Tx| {
        dispatch(win, Msg::Command(Route::Wire, Run::new(SETTINGS_READ)));
    };
    let key = format!("{key}.read-yi.{}", since.0);
    model
        .wire
        .settings
        .is_empty()
        .then(|| el().key(key).on_layout_tx(read))
}

fn view(pane: &PaneCx<'_>, _cx: &mut Cx<Win>) -> El {
    let key = pane.key(PaneKind::Settings);
    let (tokens, model) = (pane.tokens(), pane.model());
    let rows = listed(model);
    let fresh = State::default();
    let state = pane.state::<State>().unwrap_or(&fresh);
    let shown: Vec<_> = state.shown(&rows).into_iter().cloned().collect();
    let search = fields::drawn(pane, Part::Search, &state.search);
    let words = text::shown(model, pane.state::<State>());
    let as_text = fields::drawn(pane, Part::Text, &words);
    let disk = pane.hub.panes.get::<file::Disk>(None);
    let refused = disk.and_then(|disk| disk.refused.first());
    let keys = command_box::keymap(pane.hub);
    let problem = state
        .problem
        .clone()
        .or_else(|| refused.map(|why| format!("{why}; its default stays")))
        .or_else(|| keys.problems.first().map(ToString::to_string));
    let link = down(pane);
    let said = match problem {
        Some(why) => Some((why, true)),
        None if model.wire.settings.is_empty() && link.is_none() => {
            Some((WORDS.loading.what.to_owned(), false))
        }
        None => None,
    };
    let searching = Some(state.search.as_str()).filter(|words| !words.trim().is_empty());
    let look = rows::Look {
        pane: pane.pane,
        key: key.clone(),
        tokens: tokens.clone(),
        editing: state.editing.clone(),
    };
    let tint = tokens.pane_tint.alpha(if pane.focused {
        tokens.glass_pane_focus
    } else {
        tokens.glass_pane
    });
    row()
        .key(key.clone())
        .w_full()
        .h_full()
        .min_h(0.0)
        .fill(colour(tint))
        .children(read_yi(model, &key))
        .child(groups::view(&groups::Drawn {
            pane: pane.pane,
            key: &key,
            tokens,
            rows: &rows,
            state,
            search: &search,
            hint: command_box::keymap(pane.hub)
                .hint(picker::SHOW_SETTINGS)
                .unwrap_or_default(),
        }))
        .child(match link {
            Some(link) => col()
                .grow(1.0)
                .min_w(0.0)
                .h_full()
                .child(bar(pane, &key, &link, &SERVER))
                .child(rows::view(look, shown, searching, said)),
            None => rows::view(look, shown, searching, said),
        })
        .child(text::view(&text::Drawn {
            pane: pane.pane,
            key: &key,
            tokens,
            rows: &rows,
            field: &as_text,
        }))
}
