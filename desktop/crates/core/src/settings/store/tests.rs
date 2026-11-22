//! Changing a setting: the app's own in the model, Yi's as a request to the server.

use std::error::Error;

use serde_json::{Value, json};

use super::*;
use crate::msg::{Msg, step as step_msg};
use crate::settings::schema::{Choice, Control, Store};
use crate::wire::frame::Kept;

type Check = Result<(), Box<dyn Error>>;

fn density() -> Setting {
    let choice = |word: &'static str| Choice {
        label: word.into(),
        value: word.into(),
    };
    Setting {
        key: SettingKey::new("appearance.density"),
        group: "Appearance".into(),
        name: "Density".into(),
        description: "Row height and padding.".into(),
        control: Control::Select(vec![choice("dense"), choice("compact")].into()),
        store: Store::App,
        default: Some(SettingValue::Text("dense".into())),
        value: None,
    }
}

fn yi_row(id: &str, kind: Value) -> Kept<yi_types::settings::SettingRow> {
    Kept::read(
        json!({"id": id, "label": id, "tab": "Models", "help": "h", "kind": kind,
                      "applies": "live", "value": null, "default": null}),
    )
}

fn text(word: &str) -> Option<SettingValue> {
    Some(SettingValue::Text(word.to_owned().into()))
}

#[test]
fn setting_an_app_setting_changes_it_and_a_value_its_control_refuses_changes_nothing() -> Check {
    let mut model = Model::default();
    step(&mut model, SettingsMsg::Declare(vec![density()]));
    let key = SettingKey::new("appearance.density");
    step(&mut model, SettingsMsg::Set(key.clone(), text("compact")));
    assert_eq!(
        model.settings.value("appearance.density"),
        text("compact").as_ref()
    );
    step(&mut model, SettingsMsg::Set(key.clone(), text("cramped")));
    assert_eq!(
        model.settings.value("appearance.density"),
        text("compact").as_ref(),
        "a word that is no choice is not kept"
    );
    step(&mut model, SettingsMsg::Set(key, None));
    assert_eq!(
        model.settings.value("appearance.density"),
        text("dense").as_ref()
    );
    Ok(())
}

#[test]
fn setting_one_of_yis_settings_asks_the_server_and_leaves_the_model_to_its_answer() -> Check {
    let mut model = Model::default();
    model.wire.settings = vec![yi_row(
        "thinking",
        json!({"type": "choice", "options": ["off", "high"]}),
    )];
    let key = SettingKey::new("thinking");
    let effects = step_msg(
        &mut model,
        Msg::Settings(SettingsMsg::Set(key.clone(), text("high"))),
    );
    let [Effect::Wire(request)] = effects.as_slice() else {
        return Err(format!("expected one request, got {effects:?}").into());
    };
    let line: Value = serde_json::from_str(&request.line)?;
    assert_eq!(line["method"], "_yi/set_default");
    assert_eq!(line["params"], json!({"id": "thinking", "value": "high"}));
    let unknown = step(&mut model, SettingsMsg::Set(key.clone(), text("loud")));
    assert!(unknown.is_empty(), "a choice the server lacks is not sent");
    let unset = step(&mut model, SettingsMsg::Set(key, None));
    assert_eq!(unset.len(), 1, "putting the default back is sent as null");
    Ok(())
}

#[test]
fn groups_count_the_changed_settings_they_hold_not_the_ones_a_mockup_said() {
    let mut changed = density();
    changed.value = text("compact");
    let mut other = density();
    other.key = SettingKey::new("layout.chat_side");
    other.group = "Layout".into();
    let rows = [changed, density(), other];
    let groups = browse::groups(&rows);
    assert_eq!(
        groups,
        [
            browse::Group {
                name: "Appearance".into(),
                total: 2,
                changed: 1
            },
            browse::Group {
                name: "Layout".into(),
                total: 1,
                changed: 0
            },
        ]
    );
    assert_eq!(browse::changed(&rows), 1);
    assert_eq!(
        browse::search(&rows, "ROW pad").len(),
        3,
        "every word, any case"
    );
    assert!(browse::search(&rows, "glass").is_empty());
}

/// Dies with the recent folders written as one string with a literal `\n` between paths.
#[test]
fn recent_projects_are_stored_and_shown_as_a_list_and_old_text_still_reads() -> Check {
    let mut settings = Settings {
        rows: vec![PROJECTS_ROW],
    };
    let value = settings.with_project("/a");
    assert_eq!(to_json_of(&value), json!(["/a"]), "a list, not a string");
    settings.rows[0].value = Some(value);
    let value = settings.with_project("/b");
    settings.rows[0].value = Some(value);
    assert_eq!(settings.projects(), ["/b", "/a"]);
    assert_eq!(value::shown(&settings.rows[0]), "/b, /a");
    let file = json!(["/c", "/d"]);
    assert!(
        value::read(&settings.rows[0], &file).is_ok(),
        "the file's list is taken"
    );
    settings.rows[0].value = Some(SettingValue::Text("/x\n/y".into()));
    assert_eq!(
        settings.projects(),
        ["/x", "/y"],
        "the old string still reads"
    );
    Ok(())
}

fn to_json_of(value: &SettingValue) -> Value {
    value::to_json(value)
}

/// The words on what opens by itself read back as written; the chat, an unknown pane and an
/// unknown word are passed over, and taking a word back leaves the others.
#[test]
fn the_words_on_what_opens_by_itself_round_trip_and_skip_what_they_cannot_mean() {
    use crate::pane::{Opens, PaneKind, Place};
    let text = "changes never\nchat work\nbogus never\nfiles nowhere\n  agents side  \nlogs";
    let words = Opens::read(text);
    let want = [
        (PaneKind::Changes, Opens::Never),
        (PaneKind::Agents, Opens::Always(Place::Side)),
    ];
    assert_eq!(words, want.into_iter().collect());
    assert_eq!(Opens::write(&words), "changes never\nagents side");
    let mut settings = Settings::default();
    settings.rows.push(OPENS_ROW);
    assert!(settings.opens().is_empty(), "nothing said by default");
    let said = settings.with_opens(PaneKind::Web, Some(Opens::Always(Place::Shelf)));
    assert_eq!(said, SettingValue::Text("web shelf".into()));
    for row in &mut settings.rows {
        row.value = Some(SettingValue::Text(text.into()));
    }
    let gone = settings.with_opens(PaneKind::Changes, None);
    assert_eq!(gone, SettingValue::Text("agents side".into()));
}
