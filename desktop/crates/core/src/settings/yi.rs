//! Yi's own settings rows, read with `_yi/settings` and written with `_yi/set_default`.

use std::borrow::Cow;

use serde_json::Value;
use yi_types::settings::{SettingKind, SettingRow};

use crate::address::uri::Address;
use crate::command::table::{Run, Target};
use crate::ids::SettingKey;
use crate::kind::Kind;
use crate::reduce::step::commands::SETTINGS_WRITE;
use crate::settings::schema::{Choice, Control, Setting, SettingValue, Store};
use crate::settings::store::value::written;
use crate::wire::frame::Kept;

fn value_of(json: &Value) -> Option<SettingValue> {
    match json {
        Value::Null => None,
        Value::Bool(on) => Some(SettingValue::Flag(*on)),
        Value::Number(n) => n.as_f64().map(SettingValue::Number),
        Value::String(text) => Some(SettingValue::Text(text.clone().into())),
        Value::Array(_) | Value::Object(_) => Some(SettingValue::Json(json.to_string().into())),
    }
}

fn control_of(kind: &SettingKind) -> Control {
    match kind {
        SettingKind::Toggle => Control::Toggle,
        SettingKind::Choice { options } => {
            let choices = options.iter().map(|option| Choice {
                label: option.clone().into(),
                value: option.clone().into(),
            });
            Control::Select(Cow::Owned(choices.collect()))
        }
        SettingKind::Number {
            integer: false,
            min: Some(min),
            max: Some(max),
        } => match (min.as_f64(), max.as_f64()) {
            (Some(min), Some(max)) => Control::Slider {
                min,
                max,
                unit: "".into(),
            },
            _ => Control::Number,
        },
        SettingKind::Number { .. } => Control::Number,
        SettingKind::Text => Control::Text,
        SettingKind::Model => Control::Model,
        SettingKind::List => Control::Other("list".into()),
        SettingKind::Json => Control::Other("json value".into()),
        SettingKind::Other(_) => Control::Other("a kind this build does not know".into()),
    }
}

fn setting(row: &SettingRow) -> Setting {
    Setting {
        key: SettingKey::new(row.id.as_str()),
        group: row.tab.clone().into(),
        name: row.label.clone().into(),
        description: row.help.clone().into(),
        control: control_of(&row.kind),
        store: Store::Yi,
        default: value_of(&row.default),
        value: value_of(&row.value),
    }
}

/// The rows the server last sent, as settings. A row this build cannot read is left out.
pub fn rows(sent: &[Kept<SettingRow>]) -> Vec<Setting> {
    sent.iter().filter_map(Kept::known).map(setting).collect()
}

/// The command that writes `value` (`None` unsets it) as the server's default for `key`.
pub fn write(key: &SettingKey, value: Option<&SettingValue>) -> Option<Run> {
    let target = Target::Thing(Address::new(Kind::Setting, key.as_str()).ok()?);
    let text = value.map_or_else(|| "null".to_owned(), written);
    Some(Run {
        text: Some(text),
        ..Run::new(SETTINGS_WRITE).on(target)
    })
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde_json::json;

    use super::*;

    fn sent(row: Value) -> Result<Vec<Kept<SettingRow>>, Box<dyn Error>> {
        Ok(vec![Kept::read(row)])
    }

    #[test]
    fn a_choice_a_threshold_and_a_list_become_the_controls_they_are_edited_with()
    -> Result<(), Box<dyn Error>> {
        let base = |id: &str, kind: Value, value: Value| {
            json!({"id": id, "label": id, "tab": "Models", "help": "h", "kind": kind,
                   "applies": "live", "value": value, "default": null})
        };
        let mut all = sent(base(
            "thinking",
            json!({"type": "choice", "options": ["off", "high"]}),
            json!("high"),
        ))?;
        all.extend(sent(base(
            "classifier.threshold",
            json!({"type": "number", "integer": false, "min": 0, "max": 1}),
            Value::Null,
        ))?);
        all.extend(sent(base(
            "routing.order",
            json!({"type": "list"}),
            json!(3),
        ))?);
        let made = rows(&all);
        assert!(matches!(&made[0].control, Control::Select(c) if c.len() == 2));
        assert_eq!(made[0].value, Some(SettingValue::Text("high".into())));
        assert!(
            matches!(made[1].control, Control::Slider { min, max, .. } if min == 0.0 && max == 1.0)
        );
        assert!(matches!(&made[2].control, Control::Other(kind) if kind == "list"));
        assert_eq!(made[0].group, "Models", "a row's tab is its group");
        Ok(())
    }
}
