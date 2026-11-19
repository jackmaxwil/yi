//! Newtypes for every id and unit that crosses a crate boundary.

use std::borrow::Cow;
use std::fmt;

use serde::{Deserialize, Serialize};

macro_rules! text_id {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
    )+};
}

macro_rules! number_id {
    ($($(#[$doc:meta])* $name:ident),+ $(,)?) => {$(
        $(#[$doc])*
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
        pub struct $name(pub u32);
    )+};
}

text_id! {
    /// A thread: Yi's `sessionId`.
    ThreadId,
    /// A session entry, the unit rewind and fork name.
    EntryId,
    /// A permission ask, `perm_<session>_<n>` as the server numbers it.
    ApprovalId,
    /// A child agent.
    AgentId,
    /// A row of a feed's table: stable across updates, never a position.
    RowId,
    /// A theme file's id, such as `graphite`.
    ThemeId,
}

/// A setting's dotted name, such as `appearance.glass`; its first part is its section.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SettingKey(Cow<'static, str>);

impl SettingKey {
    /// A key written in code, for a setting declared in a static table.
    pub const fn fixed(key: &'static str) -> Self {
        Self(Cow::Borrowed(key))
    }

    pub fn new(key: impl Into<String>) -> Self {
        Self(Cow::Owned(key.into()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

number_id! {
    /// A pane instance; it keeps its id when it moves between windows.
    PaneId,
    /// A window of the app.
    WindowId,
    /// A tab inside a pane.
    TabId,
}

/// A command's dotted name, such as `layout.flip-chat`; declared beside the command.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct CommandId(pub &'static str);

/// A feed: the source's name and which instance of it (a terminal tab, a language server).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct FeedId {
    pub source: &'static str,
    pub instance: u32,
}

impl FeedId {
    pub const fn new(source: &'static str) -> Self {
        Self {
            source,
            instance: 0,
        }
    }

    pub const fn instance(self, instance: u32) -> Self {
        Self { instance, ..self }
    }
}

/// Milliseconds on the app's injected clock.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Default)]
pub struct Millis(pub u64);

impl Millis {
    /// Since `then`, as the sidebar and the bell write it: `now`, `11m`, `2h`, `1d`, `1w`.
    pub fn age(self, then: Millis) -> String {
        let seconds = self.0.saturating_sub(then.0) / 1000;
        match seconds {
            0..60 => "now".to_owned(),
            60..3_600 => format!("{}m", seconds / 60),
            3_600..86_400 => format!("{}h", seconds / 3_600),
            86_400..604_800 => format!("{}d", seconds / 86_400),
            _ => format!("{}w", seconds / 604_800),
        }
    }
}
