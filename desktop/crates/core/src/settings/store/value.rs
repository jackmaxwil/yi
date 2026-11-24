//! How a control stores its value, and how a value is read from and written as JSON.

use serde_json::{Number, Value};

use crate::ids::SettingKey;
use crate::settings::schema::{Choice, Control, Setting, SettingValue};

/// A value as JSON: how the file and the server hold it.
pub type Json = Value;

/// A value a setting's control cannot hold, named by its key and what the control takes.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{} takes {wants}", key.as_str())]
pub struct Refused {
    pub key: SettingKey,
    pub wants: String,
}

/// A whole number is written without a fraction: Yi refuses `60.0` for an integer setting.
pub fn to_json(value: &SettingValue) -> Value {
    match value {
        SettingValue::Flag(on) => Value::Bool(*on),
        SettingValue::Number(n) if n.fract() == 0.0 && n.abs() < 1e15 => Value::from(*n as i64),
        SettingValue::Number(n) => Number::from_f64(*n).map_or(Value::Null, Value::Number),
        SettingValue::Text(text) => Value::String(text.to_string()),
        SettingValue::Json(raw) => serde_json::from_str(raw).unwrap_or(Value::Null),
    }
}

/// A value as the text view writes it.
pub fn written(value: &SettingValue) -> String {
    to_json(value).to_string()
}

/// What the control takes, for a refusal to say.
pub fn wants(control: &Control) -> String {
    let among = |choices: &[Choice]| {
        let values: Vec<&str> = choices.iter().map(|c| c.value.as_ref()).collect();
        format!("one of {}", values.join(", "))
    };
    match control {
        Control::Toggle => "true or false".to_owned(),
        Control::Select(choices) | Control::Segmented(choices) | Control::Theme(choices) => {
            among(choices)
        }
        Control::Slider { min, max, .. } => format!("a number from {min} to {max}"),
        Control::Number => "a number".to_owned(),
        Control::Colour => "a colour such as #78d2c6".to_owned(),
        Control::Text => "text".to_owned(),
        Control::Model => "a model written provider/name".to_owned(),
        Control::Other(kind) => format!("a {kind}, which is edited in Yi's config file"),
    }
}

fn is_colour(text: &str) -> bool {
    let digits = text.strip_prefix('#').unwrap_or("");
    matches!(digits.len(), 6 | 8) && digits.chars().all(|c| c.is_ascii_hexdigit())
}

/// `json` as the value of `setting`, or why its control cannot hold it.
pub fn read(setting: &Setting, json: &Value) -> Result<SettingValue, Refused> {
    let found = match (&setting.control, json) {
        (Control::Toggle, Value::Bool(on)) => Some(SettingValue::Flag(*on)),
        (
            Control::Select(choices) | Control::Segmented(choices) | Control::Theme(choices),
            Value::String(text),
        ) => choices
            .iter()
            .any(|c| c.value == text.as_str())
            .then(|| SettingValue::Text(text.clone().into())),
        (Control::Slider { min, max, .. }, Value::Number(n)) => n
            .as_f64()
            .filter(|n| (*min..=*max).contains(n))
            .map(SettingValue::Number),
        (Control::Number, Value::Number(n)) => n.as_f64().map(SettingValue::Number),
        (Control::Colour, Value::String(text)) => {
            is_colour(text).then(|| SettingValue::Text(text.clone().into()))
        }
        (Control::Other(_), Value::Array(_)) => Some(SettingValue::Json(json.to_string().into())),
        (Control::Text, Value::String(text)) => Some(SettingValue::Text(text.clone().into())),
        (Control::Model, Value::String(text)) => text
            .split_once('/')
            .filter(|(provider, name)| !provider.is_empty() && !name.is_empty())
            .map(|_| SettingValue::Text(text.clone().into())),
        _ => None,
    };
    found.ok_or_else(|| Refused {
        key: setting.key.clone(),
        wants: wants(&setting.control),
    })
}

/// Whether the control of `setting` can hold `value`.
pub fn fits(setting: &Setting, value: &SettingValue) -> bool {
    read(setting, &to_json(value)).is_ok()
}

/// Words typed into a row's own field, as the value of `setting`.
pub fn typed(setting: &Setting, words: &str) -> Result<SettingValue, Refused> {
    let words = words.trim();
    let json = match setting.control {
        Control::Number | Control::Slider { .. } => words
            .parse::<f64>()
            .ok()
            .and_then(Number::from_f64)
            .map_or(Value::Null, Value::Number),
        _ => Value::String(words.to_owned()),
    };
    read(setting, &json)
}

fn number(n: f64) -> String {
    match to_json(&SettingValue::Number(n)) {
        Value::Null => String::new(),
        json => json.to_string(),
    }
}

/// A slider's value as its reading says it: a share of the range for `%`, else with its unit.
pub fn reading(control: &Control, value: f64) -> String {
    match control {
        Control::Slider { min, max, unit } if unit == "%" && max > min => {
            format!("{}%", number(((value - min) / (max - min) * 100.0).round()))
        }
        Control::Slider { unit, .. } if !unit.is_empty() => format!("{} {unit}", number(value)),
        _ => number(value),
    }
}

/// A value in words, as a row shows it. `label` names the choice a text value stands for.
pub fn shown(setting: &Setting) -> String {
    let choices = match &setting.control {
        Control::Select(c) | Control::Segmented(c) | Control::Theme(c) => Some(c),
        _ => None,
    };
    match setting.effective() {
        None => "not set".to_owned(),
        Some(SettingValue::Flag(on)) => if *on { "on" } else { "off" }.to_owned(),
        Some(SettingValue::Number(n)) => match &setting.control {
            Control::Other(_) => format!("{} set", number(*n)),
            control => reading(control, *n),
        },
        Some(SettingValue::Text(text)) => choices
            .and_then(|choices| choices.iter().find(|choice| choice.value == *text))
            .map_or_else(|| text.to_string(), |choice| choice.label.to_string()),
        Some(SettingValue::Json(raw)) => match serde_json::from_str::<Vec<String>>(raw) {
            Ok(list) => list.join(", "),
            Err(_) => raw.to_string(),
        },
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use serde_json::json;

    use super::*;
    use crate::settings::schema::Store;

    fn row(control: Control) -> Setting {
        Setting {
            key: SettingKey::new("appearance.glass"),
            group: "Appearance".into(),
            name: "See-through".into(),
            description: "".into(),
            control,
            store: Store::App,
            default: None,
            value: None,
        }
    }

    #[test]
    fn a_slider_refuses_a_number_off_its_range_and_names_the_range() -> Result<(), Box<dyn Error>> {
        let glass = row(Control::Slider {
            min: 0.0,
            max: 1.0,
            unit: "%".into(),
        });
        assert_eq!(
            read(&glass, &json!(0.6))?,
            SettingValue::Number(0.6),
            "a value in range is kept"
        );
        let refused = read(&glass, &json!(1.5)).err().ok_or("1.5 was accepted")?;
        assert_eq!(
            refused.to_string(),
            "appearance.glass takes a number from 0 to 1"
        );
        Ok(())
    }

    #[test]
    fn a_whole_number_is_written_without_a_fraction() {
        assert_eq!(written(&SettingValue::Number(8760.0)), "8760");
        assert_eq!(written(&SettingValue::Number(0.6)), "0.6");
    }

    #[test]
    fn a_colour_needs_six_or_eight_hex_digits() {
        let colour = row(Control::Colour);
        assert!(read(&colour, &json!("#78d2c6")).is_ok());
        assert!(read(&colour, &json!("#78d2c")).is_err());
        assert!(read(&colour, &json!("78d2c6")).is_err());
    }

    #[test]
    fn a_percent_slider_reads_as_its_share_of_the_range() {
        let slider = Control::Slider {
            min: 0.0,
            max: 1.0,
            unit: "%".into(),
        };
        assert_eq!(reading(&slider, 0.6), "60%");
        assert_eq!(reading(&slider, 0.855), "86%");
    }
}
