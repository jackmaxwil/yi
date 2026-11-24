//! What a setting is, whoever stores it (record 7.3). Every text is a `Cow`, so one type
//! serves a row declared in a static table and a row read from Yi.

use std::borrow::Cow;

use crate::ids::SettingKey;

pub type Words = Cow<'static, str>;

/// Where a setting is kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Store {
    /// The app's own file: look, layouts, keys.
    App,
    /// Yi's config, read and written through the server.
    Yi,
}

/// One choice of a select or a segmented control: what it says and what is stored.
#[derive(Debug, Clone, PartialEq)]
pub struct Choice {
    pub label: Words,
    pub value: Words,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    Toggle,
    Select(Cow<'static, [Choice]>),
    Segmented(Cow<'static, [Choice]>),
    /// A select of theme names with an Edit chip beside it that opens Themes.
    Theme(Cow<'static, [Choice]>),
    /// A range shown in `unit`, stored as the number itself.
    Slider {
        min: f64,
        max: f64,
        unit: Words,
    },
    Colour,
    Text,
    Number,
    /// A model picker.
    Model,
    /// A control this app does not draw; the row shows its value as text.
    Other(Words),
}

#[derive(Debug, Clone, PartialEq)]
pub enum SettingValue {
    Flag(bool),
    Number(f64),
    Text(Words),
    /// Anything else, as the JSON it was written in.
    Json(Words),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Setting {
    pub key: SettingKey,
    /// The group in the settings tree, such as `Appearance`.
    pub group: Words,
    pub name: Words,
    pub description: Words,
    pub control: Control,
    pub store: Store,
    pub default: Option<SettingValue>,
    /// `None` is unset: the default applies.
    pub value: Option<SettingValue>,
}

impl Setting {
    /// The value in effect: what is stored, else the default.
    pub fn effective(&self) -> Option<&SettingValue> {
        self.value.as_ref().or(self.default.as_ref())
    }

    /// Whether the value in effect differs from the default.
    pub fn changed(&self) -> bool {
        self.value.is_some() && self.value != self.default
    }
}
