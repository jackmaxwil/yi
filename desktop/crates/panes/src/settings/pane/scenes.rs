//! The pane's scenes: the mockup's ten settings stepped into the model, a search, a line
//! that does not parse, and Yi's own settings from a recording.

use std::borrow::Cow;

use yd_core::msg::{Msg, step};
use yd_core::pane::PaneKind;
use yd_core::row::{Row, Value, rows_from_json};
use yd_core::settings::schema::{Choice, Control, Setting, SettingValue, Store};
use yd_core::settings::store;
use yd_core::settings::store::SettingsMsg;
use yd_core::settings::store::text::render;
use yd_core::wire::replay::play;
use yd_ui::pane::{SCENE_PANE, Scene, Stage};
use yd_ui::win::Hub;

use super::state::{Draft, State, Tab};

const MOCKUP: &str = include_str!("../../../../../fixtures/mockup/settings.rows.json");
const LISTS: &str = include_str!("../../../../../fixtures/wire/lists.jsonl");
/// The recording's lines up to the server's answer to `_yi/settings`.
const SETTINGS_READ: usize = 41;

pub const SCENES: &[Scene] = &[
    scene("settings/mockup", mockup),
    scene("settings/search", search),
    scene("settings/bad-line", bad_line),
    scene("settings/yi", yi),
    // The same pane under the fixtures of its two side columns: parity reads a scene by name.
    scene("settings-groups/mockup", mockup),
    scene("settings-text/mockup", mockup),
];

const fn scene(name: &'static str, setup: fn(&mut Hub)) -> Scene {
    Scene {
        name,
        stage: Stage::Pane(PaneKind::Settings),
        setup,
    }
}

const fn choice(label: &'static str, value: &'static str) -> Choice {
    Choice {
        label: Cow::Borrowed(label),
        value: Cow::Borrowed(value),
    }
}

const fn text(value: &'static str) -> SettingValue {
    SettingValue::Text(Cow::Borrowed(value))
}

/// What the mockup draws but its rows do not say: how each setting is set, and its default.
/// It marks seven rows changed and names no defaults: these make the same seven differ.
const SPECS: &[(&str, Control, SettingValue)] = &[
    (
        "appearance.theme",
        Control::Theme(Cow::Borrowed(&[
            choice("Yi Dark", "Yi Dark"),
            choice("Graphite", "Graphite"),
            choice("Ink", "Ink"),
            choice("High contrast", "High contrast"),
            choice("Terminal", "Terminal"),
        ])),
        text("Yi Dark"),
    ),
    (
        "appearance.density",
        Control::Segmented(Cow::Borrowed(&[
            choice("Dense", "dense"),
            choice("Compact", "compact"),
            choice("Roomy", "roomy"),
        ])),
        text("dense"),
    ),
    (
        "appearance.text_font",
        Control::Select(Cow::Borrowed(&[
            choice("System", "system"),
            choice("Geist", "geist"),
        ])),
        text("geist"),
    ),
    (
        "appearance.code_font",
        Control::Select(Cow::Borrowed(&[
            choice("Geist Mono", "Geist Mono"),
            choice("System mono", "System mono"),
        ])),
        text("System mono"),
    ),
    (
        "appearance.glass",
        Control::Slider {
            min: 0.0,
            max: 1.0,
            unit: Cow::Borrowed("%"),
        },
        SettingValue::Number(0.2),
    ),
    (
        "layout.chat_side",
        Control::Segmented(Cow::Borrowed(&[
            choice("Left", "left"),
            choice("Right", "right"),
        ])),
        text("right"),
    ),
    (
        "layout.sidebar_folded",
        Control::Select(Cow::Borrowed(&[
            choice("Full titles", "titles"),
            choice("Icons only", "icons"),
        ])),
        text("icons"),
    ),
    ("appearance.needs_you", Control::Colour, text("#6cc4b8")),
    ("motion.springs", Control::Toggle, SettingValue::Flag(true)),
    (
        "motion.moving_light",
        Control::Toggle,
        SettingValue::Flag(true),
    ),
];

fn capital(word: &str) -> String {
    let mut chars = word.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// A mockup row as a setting: its words and value from the row, the rest from [`SPECS`].
fn setting(row: &Row) -> Option<Setting> {
    let key = row.words("key")?;
    let (_, control, default) = SPECS.iter().find(|(each, ..)| *each == key)?;
    let choices = match control {
        Control::Select(c) | Control::Segmented(c) | Control::Theme(c) => Some(c),
        _ => None,
    };
    let value = match row.get("value")? {
        Value::Flag(on) => SettingValue::Flag(*on),
        Value::Meter { ratio, .. } => SettingValue::Number(ratio.to_string().parse().ok()?),
        Value::Text(label) => match choices {
            Some(choices) => {
                SettingValue::Text(choices.iter().find(|c| c.label == *label)?.value.clone())
            }
            None => SettingValue::Text(label.clone().into()),
        },
        _ => return None,
    };
    Some(Setting {
        key: yd_core::ids::SettingKey::new(key),
        group: capital(key.split_once('.')?.0).into(),
        name: row.words("setting")?.to_owned().into(),
        description: row.words("description")?.to_owned().into(),
        control: control.clone(),
        store: Store::App,
        default: Some(default.clone()),
        value: Some(value),
    })
}

fn state(hub: &mut Hub) -> Option<&mut State> {
    hub.panes.get_mut::<State>(Some(SCENE_PANE))
}

fn mockup(hub: &mut Hub) {
    let rows = rows_from_json(MOCKUP).unwrap_or_default();
    let declared: Vec<Setting> = rows.iter().filter_map(setting).collect();
    step(
        &mut hub.model,
        Msg::Settings(SettingsMsg::Declare(declared)),
    );
    if let Some(state) = state(hub) {
        state.tab = Tab::All;
        state.group = Some("Appearance".to_owned());
    }
}

fn search(hub: &mut Hub) {
    mockup(hub);
    if let Some(state) = state(hub) {
        state.search = "font".to_owned();
    }
}

/// The text view after a value was typed past its slider's range.
fn bad_line(hub: &mut Hub) {
    mockup(hub);
    let rendered = render(&store::all(&hub.model));
    let text = rendered.replace("= 0.6", "= 60");
    if let Some(state) = state(hub) {
        state.draft = Some(Draft { text, rendered });
    }
}

fn yi(hub: &mut Hub) {
    let recording: Vec<&str> = LISTS.lines().take(SETTINGS_READ).collect();
    if play(&mut hub.model, &recording.join("\n")).is_err() {
        return;
    }
    if let Some(state) = state(hub) {
        state.group = Some("Permissions".to_owned());
    }
}
