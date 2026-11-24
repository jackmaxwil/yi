//! What the pane keeps between draws: the group and tab in front, the row being edited and
//! what the text view last applied.

use yd_core::ids::{SettingKey, TabId};
use yd_core::settings::schema::{Setting, SettingValue};
use yd_core::settings::store::browse;

/// The three tabs of the bar: the chosen group, every setting, the ones that differ.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Group,
    All,
    Changed,
}

impl Tab {
    pub const fn id(self) -> TabId {
        TabId(self as u32)
    }

    pub fn of(id: TabId) -> Option<Tab> {
        [Tab::Group, Tab::All, Tab::Changed]
            .into_iter()
            .find(|tab| tab.id() == id)
    }
}

#[derive(Debug, Clone, Default)]
pub struct State {
    pub group: Option<String>,
    pub tab: Tab,
    /// The text, number or colour row being typed into.
    pub editing: Option<SettingKey>,
    /// The search box's words; the window's own field holds them while it has the keyboard.
    pub search: String,
    /// The text view as last typed, with the text the model gave when it was typed. It is
    /// shown only while the model still gives that text: a change made anywhere else wins.
    pub draft: Option<Draft>,
    /// The values the text view last applied; none until it has applied any.
    pub heard: Option<Vec<(SettingKey, SettingValue)>>,
    /// Why the last change could not be kept, said under the rows.
    pub problem: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Draft {
    pub text: String,
    pub rendered: String,
}

impl State {
    /// The group in front: the one picked if it is still listed, else the first.
    pub fn group_in<'a>(&'a self, rows: &'a [Setting]) -> Option<String> {
        let groups = browse::groups(rows);
        let picked = self
            .group
            .as_ref()
            .filter(|name| groups.iter().any(|g| &g.name == *name));
        picked.or(groups.first().map(|g| &g.name)).cloned()
    }

    /// The settings the middle column lists: every match of a search, else the tab's.
    pub fn shown<'a>(&self, rows: &'a [Setting]) -> Vec<&'a Setting> {
        if !self.search.trim().is_empty() {
            return browse::search(rows, &self.search);
        }
        match self.tab {
            Tab::All => rows.iter().collect(),
            Tab::Changed => rows.iter().filter(|row| row.changed()).collect(),
            Tab::Group => {
                let group = self.group_in(rows);
                rows.iter()
                    .filter(|row| Some(row.group.to_string()) == group)
                    .collect()
            }
        }
    }
}

/// The pane's three text fields, each under its own name in the window's field map.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Part {
    Search,
    Text,
    Row,
}

impl Part {
    pub const fn name(self) -> &'static str {
        match self {
            Part::Search => "search",
            Part::Text => "text",
            Part::Row => "row",
        }
    }
}
