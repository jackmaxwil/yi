//! The settings page's rows: every `config.json` leaf, how it is edited, and when a saved
//! change takes hold. The daemon fills each row's value from the file and refuses an edit here.

use serde::{Deserialize, Serialize};
use serde_json::{Number, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Applies {
    Live,
    NextSession,
    DaemonRestart,
    SoloTui,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", tag = "type")]
pub enum SettingKind {
    Toggle,
    Choice {
        options: Vec<String>,
    },
    Number {
        integer: bool,
        min: Option<Number>,
        max: Option<Number>,
    },
    Text,
    Model,
    List,
    Json,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingRow {
    pub id: String,
    pub label: String,
    pub tab: String,
    pub help: String,
    pub kind: SettingKind,
    pub applies: Applies,
    /// The file's value; `null` is unset, and the help names what unset means. The daemon sends
    /// a list or JSON row's entry count instead, so a key stored in the wrong place stays put.
    #[serde(default)]
    pub value: Value,
    /// What an unset toggle means, so the page flips the value in effect, not the stored one.
    #[serde(default)]
    pub default: Value,
}

pub const TABS: [&str; 6] = [
    "Models",
    "Permissions",
    "Tools",
    "Agents",
    "Budget",
    "Interface",
];

/// Every leaf of [`crate::config::UserConfig`], in page order; the table is data so a row is
/// one line of `settings.json`, and a table that fails to parse is no rows, which a test refuses.
pub fn rows() -> Vec<SettingRow> {
    serde_json::from_str(include_str!("settings.json")).unwrap_or_default()
}

/// Why `value` cannot be saved under `row`; `null` (unset) always can.
pub fn refusal(row: &SettingRow, value: &Value) -> Option<String> {
    let fits = match (&row.kind, value) {
        (_, Value::Null) | (SettingKind::Json, _) => true,
        (SettingKind::Toggle, Value::Bool(_)) => true,
        (SettingKind::Choice { options }, Value::String(text)) => options.contains(text),
        (SettingKind::Text, Value::String(_)) => true,
        (SettingKind::Model, Value::String(text)) => text
            .split_once('/')
            .is_some_and(|(p, m)| !p.is_empty() && !m.is_empty()),
        (SettingKind::List, Value::Array(items)) => items.iter().all(Value::is_string),
        (SettingKind::Number { integer, min, max }, Value::Number(number)) => {
            return number_refusal(*integer, min.as_ref(), max.as_ref(), number)
                .map(|why| format!("{} takes {why}", row.id));
        }
        _ => false,
    };
    (!fits).then(|| format!("{} takes {}", row.id, describe(&row.kind)))
}

fn number_refusal(
    integer: bool,
    min: Option<&Number>,
    max: Option<&Number>,
    number: &Number,
) -> Option<String> {
    let as_f64 = |n: &Number| n.as_f64().unwrap_or(f64::NAN);
    if integer && number.as_u64().is_none() {
        return Some("a whole number".to_owned());
    }
    let low = min.is_some_and(|min| as_f64(number) < as_f64(min));
    let high = max.is_some_and(|max| as_f64(number) > as_f64(max));
    match (low, high, min, max) {
        (true, _, Some(min), _) => Some(format!("at least {min}")),
        (_, true, _, Some(max)) => Some(format!("at most {max}")),
        _ => None,
    }
}

fn describe(kind: &SettingKind) -> String {
    match kind {
        SettingKind::Toggle => "true or false".to_owned(),
        SettingKind::Choice { options } => format!("one of {}", options.join(", ")),
        SettingKind::Number { integer: true, .. } => "a whole number".to_owned(),
        SettingKind::Number { integer: false, .. } => "a number".to_owned(),
        SettingKind::Text => "text".to_owned(),
        SettingKind::Model => "provider/model".to_owned(),
        SettingKind::List => "a list of text".to_owned(),
        SettingKind::Json => "any JSON".to_owned(),
    }
}

/// The value at `id`'s dotted path in a raw config.
pub fn lookup<'a>(config: &'a Value, id: &str) -> &'a Value {
    id.split('.')
        .try_fold(config, |at, key| at.get(key))
        .unwrap_or(&Value::Null)
}
