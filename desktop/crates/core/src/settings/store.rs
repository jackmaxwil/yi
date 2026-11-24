//! The settings the app holds, and a change to one.

pub mod browse;
pub mod text;
pub mod value;

use crate::ids::SettingKey;
use crate::model::Model;
use crate::msg::Effect;
use crate::pane::{Opens, PaneKind};
use crate::reduce::step::request;
use std::borrow::Cow;
use std::collections::BTreeMap;

use crate::settings::schema::{Control, Setting, SettingValue, Store};
use crate::settings::yi;

/// The project folders the app knows, newest first, as a list.
pub const PROJECTS: &str = "projects.recent";
/// Invariant: each poll asks the server for each folder's threads, so this bounds the lists
/// one poll sends at one more than it.
pub const MAX_PROJECTS: usize = 8;
pub const PROJECTS_ROW: Setting = Setting {
    key: SettingKey::fixed(PROJECTS),
    group: Cow::Borrowed("Projects"),
    name: Cow::Borrowed("Recent project folders"),
    description: Cow::Borrowed("Folders a thread was started in or chosen for one, a list."),
    control: Control::Other(Cow::Borrowed("list of folders")),
    store: Store::App,
    default: Some(SettingValue::Json(Cow::Borrowed("[]"))),
    value: None,
};

/// Which kinds of pane open by themselves, over their own rules: [`Opens::read`]'s text.
pub const OPENS: &str = "panes.opens";
pub const OPENS_ROW: Setting = Setting {
    key: SettingKey::fixed(OPENS),
    group: Cow::Borrowed("Layout"),
    name: Cow::Borrowed("Panes that open by themselves"),
    description: Cow::Borrowed(
        "One line a pane: its name, a space, then never, or the place it is always open in \
         (work, side, shelf, chat).",
    ),
    control: Control::Text,
    store: Store::App,
    default: Some(SettingValue::Text(Cow::Borrowed(""))),
    value: None,
};

/// The app's own settings. Yi's are the server's rows, read through [`all`].
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Settings {
    pub rows: Vec<Setting>,
}

impl Settings {
    /// The value in effect for `key`, if any row declares it.
    pub fn value(&self, key: &str) -> Option<&SettingValue> {
        self.rows
            .iter()
            .find(|row| row.key.as_str() == key)?
            .effective()
    }

    /// The person's word on each kind of pane opening by itself.
    pub fn opens(&self) -> BTreeMap<PaneKind, Opens> {
        match self.value(OPENS) {
            Some(SettingValue::Text(text)) => Opens::read(text),
            _ => BTreeMap::new(),
        }
    }

    /// The [`OPENS`] value with `kind`'s word set, or gone.
    pub fn with_opens(&self, kind: PaneKind, word: Option<Opens>) -> SettingValue {
        let mut words = self.opens();
        match word {
            Some(word) => words.insert(kind, word),
            None => words.remove(&kind),
        };
        SettingValue::Text(Cow::Owned(Opens::write(&words)))
    }

    /// The known project folders, newest first, at most [`MAX_PROJECTS`].
    pub fn projects(&self) -> Vec<String> {
        let all: Vec<String> = match self.value(PROJECTS) {
            Some(SettingValue::Json(raw)) => serde_json::from_str(raw).unwrap_or_default(),
            // A file written before the list was one: a line a folder.
            Some(SettingValue::Text(text)) => text.lines().map(str::to_owned).collect(),
            _ => Vec::new(),
        };
        let kept = all
            .iter()
            .map(|line| line.trim())
            .filter(|line| !line.is_empty());
        kept.take(MAX_PROJECTS).map(str::to_owned).collect()
    }

    /// The projects value with `folder` first, once, and the oldest past the cap gone.
    pub fn with_project(&self, folder: &str) -> SettingValue {
        let others = self.projects().into_iter().filter(|known| known != folder);
        let kept: Vec<String> = std::iter::once(folder.to_owned()).chain(others).collect();
        projects_value(kept.get(..MAX_PROJECTS).unwrap_or(&kept))
    }
}

/// The value [`PROJECTS`] holds for these folders.
pub fn projects_value(folders: &[String]) -> SettingValue {
    SettingValue::Json(Cow::Owned(
        serde_json::to_string(folders).unwrap_or_default(),
    ))
}

/// Every setting the pane lists: the app's own, then Yi's as the server last sent them.
pub fn all(model: &Model) -> Vec<Setting> {
    let mut rows = model.settings.rows.clone();
    rows.extend(yi::rows(&model.wire.settings));
    rows
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingsMsg {
    /// Rows to list: a row already listed under the same key is replaced.
    Declare(Vec<Setting>),
    /// Set one value; `None` puts the default back.
    Set(SettingKey, Option<SettingValue>),
}

pub fn step(model: &mut Model, change: SettingsMsg) -> Vec<Effect> {
    match change {
        SettingsMsg::Declare(rows) => {
            for row in rows {
                match model
                    .settings
                    .rows
                    .iter_mut()
                    .find(|each| each.key == row.key)
                {
                    Some(held) => *held = row,
                    None => model.settings.rows.push(row),
                }
            }
            Vec::new()
        }
        SettingsMsg::Set(key, value) => set(model, &key, value),
    }
}

fn set(model: &mut Model, key: &SettingKey, value: Option<SettingValue>) -> Vec<Effect> {
    let fits = |row: &Setting| value.as_ref().is_none_or(|value| value::fits(row, value));
    if let Some(row) = model.settings.rows.iter_mut().find(|row| &row.key == key) {
        if fits(row) {
            row.value = value;
        }
        return Vec::new();
    }
    let asked = yi::rows(&model.wire.settings)
        .into_iter()
        .find(|row| &row.key == key)
        .filter(fits)
        .and_then(|_| yi::write(key, value.as_ref()));
    asked.map_or_else(Vec::new, |run| request(model, &run))
}

#[cfg(test)]
mod tests;
