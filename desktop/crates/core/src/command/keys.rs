//! The shortcut map: every command's key, the user's own keys over the table's, grouped as
//! the Mac menus group them. Matching, hints, the help sheet and the Keys settings read it.

use std::borrow::Cow;
use std::collections::BTreeSet;

use crate::command::table::{Command, Commands, MENUS, keycap, normal};
use crate::ids::{CommandId, SettingKey};
use crate::settings::schema::{Control, Setting, SettingValue, Store};
use crate::settings::store::Settings;

/// A setting under this prefix and a command's id is the user's key for it.
const PREFIX: &str = "keys.";
/// Where a command that names no menu is listed.
const OTHER: &str = "More";
/// What the user writes to leave a command with no key.
pub const NONE: &str = "none";

pub fn setting_key(command: CommandId) -> SettingKey {
    SettingKey::new([PREFIX, command.0].concat())
}

/// A key the map would not take, said as Settings says it.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum Problem {
    #[error("{}{}: {text:?} is not a key, such as cmd+shift+k; the default stays", PREFIX, command.0)]
    NotAKey { command: CommandId, text: String },
    #[error("{}{}: {key} is already {}'s; it keeps no key", PREFIX, refused.0, kept.0)]
    Taken {
        key: String,
        kept: CommandId,
        refused: CommandId,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Row {
    pub command: CommandId,
    pub title: &'static str,
    /// The Mac menu it is listed in.
    pub group: &'static str,
    /// The key in force, as Rune parses it.
    pub key: Option<String>,
    /// The user's key is not the table's.
    pub changed: bool,
}

/// From a key to the one command it runs, and from a command to the key it shows.
#[derive(Debug, Clone, Default)]
pub struct Map {
    rows: Vec<Row>,
    pub problems: Vec<Problem>,
}

impl Map {
    /// The table's keys with the user's `keys.<command id>` over them; `valid` says whether
    /// a text is a key. A bad text or a taken key is a problem, never silent.
    pub fn new(commands: &Commands, settings: &Settings, valid: impl Fn(&str) -> bool) -> Self {
        let mut seen = BTreeSet::new();
        let mut map = Self::default();
        for command in commands.iter().filter(|command| seen.insert(command.id)) {
            let asked = settings.value(setting_key(command.id).as_str());
            let asked = match asked {
                Some(SettingValue::Text(text)) => Some(text.trim()),
                _ => None,
            };
            let (key, changed) = match asked {
                None => (command.key.map(str::to_owned), false),
                Some(text) if text.is_empty() || text.eq_ignore_ascii_case(NONE) => {
                    (None, command.key.is_some())
                }
                Some(text) if valid(text) => (
                    Some(text.to_owned()),
                    command.key.map(normal) != Some(normal(text)),
                ),
                Some(text) => {
                    let (id, text) = (command.id, text.to_owned());
                    map.problems.push(Problem::NotAKey { command: id, text });
                    (command.key.map(str::to_owned), false)
                }
            };
            map.rows.push(Row {
                command: command.id,
                title: command.title,
                group: command.menu.unwrap_or(OTHER),
                key,
                changed,
            });
        }
        map.settle();
        map
    }

    /// Invariant: a key runs one command whatever is focused, so a later holder loses it;
    /// the table's keys are kept before the user's, so a changed key never costs a default.
    fn settle(&mut self) {
        let mut order: Vec<usize> = (0..self.rows.len()).collect();
        order.sort_by_key(|at| self.rows.get(*at).is_some_and(|row| row.changed));
        let mut held: Vec<(String, CommandId)> = Vec::new();
        for at in order {
            let Some(row) = self.rows.get_mut(at) else {
                continue;
            };
            let Some(key) = row.key.as_deref().map(normal) else {
                continue;
            };
            let owner = held.iter().find(|(each, _)| *each == key);
            if let Some((_, kept)) = owner {
                let (kept, refused) = (*kept, row.command);
                row.key = None;
                self.problems.push(Problem::Taken { key, kept, refused });
            } else {
                held.push((key, row.command));
            }
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The key in force for `command`, as Rune parses it.
    pub fn key(&self, command: CommandId) -> Option<&str> {
        let row = self.rows.iter().find(|row| row.command == command);
        row.and_then(|row| row.key.as_deref())
    }

    /// The key as a hint draws it: `⌘K`.
    pub fn hint(&self, command: CommandId) -> Option<String> {
        self.key(command).map(keycap)
    }

    /// Every binding: the key and its command.
    pub fn bound(&self) -> impl Iterator<Item = (&str, CommandId)> + '_ {
        let keyed = self.rows.iter();
        keyed.filter_map(|row| Some((row.key.as_deref()?, row.command)))
    }

    /// The command bound to `key`, however its modifiers are ordered.
    pub fn command(&self, key: &str) -> Option<CommandId> {
        let key = normal(key);
        self.bound()
            .find(|(bound, _)| normal(bound) == key)
            .map(|(_, id)| id)
    }

    /// The commands with a key, in the order of the Mac menus: the menus named in
    /// [`MENUS`] first, then any other, each with its commands in table order.
    pub fn groups(&self) -> Vec<(&'static str, Vec<&Row>)> {
        let mut names: Vec<&'static str> = MENUS.to_vec();
        for row in &self.rows {
            if !names.contains(&row.group) {
                names.push(row.group);
            }
        }
        let keyed = |name: &str| -> Vec<&Row> {
            let rows = self.rows.iter();
            rows.filter(|row| row.group == name && row.key.is_some())
                .collect()
        };
        let groups = names.into_iter().map(|name| (name, keyed(name)));
        groups.filter(|(_, rows)| !rows.is_empty()).collect()
    }
}

/// The Keys group of Settings: a text row for every command the app can run on its own
/// (aimed at nothing) and every command with a key, whose default is the table's key.
pub fn settings(commands: &Commands) -> Vec<Setting> {
    let mut seen = BTreeSet::new();
    let own = |command: &&Command| command.kinds.is_empty() || command.key.is_some();
    let rows = commands.iter().filter(own);
    let row = |command: &'static Command| Setting {
        key: setting_key(command.id),
        group: Cow::Borrowed("Keys"),
        name: Cow::Borrowed(command.title),
        description: Cow::Owned(format!(
            "{} menu. A key such as cmd+shift+k, or {NONE}.",
            command.menu.unwrap_or(OTHER)
        )),
        control: Control::Text,
        store: Store::App,
        default: Some(SettingValue::Text(Cow::Borrowed(
            command.key.unwrap_or(NONE),
        ))),
        value: None,
    };
    rows.filter(|command| seen.insert(command.id))
        .map(row)
        .collect()
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;
    use crate::command::table::{Route, declared};
    use crate::kind::Kind;

    const fn row(id: &'static str, key: &'static str, kinds: &'static [Kind]) -> Command {
        Command {
            id: CommandId(id),
            title: id,
            key: Some(key),
            menu: Some("View"),
            kinds,
            route: Route::Model,
        }
    }

    fn any(_: &str) -> bool {
        true
    }

    fn table(rows: &'static [Command]) -> Commands {
        Commands::new(rows)
    }

    fn user(keys: &[(&str, &str)]) -> Settings {
        let rows = keys.iter().map(|(id, text)| Setting {
            value: Some(SettingValue::Text(text.to_string().into())),
            key: SettingKey::new(format!("{PREFIX}{id}")),
            group: "Keys".into(),
            name: "".into(),
            description: "".into(),
            control: Control::Text,
            store: Store::App,
            default: None,
        });
        Settings {
            rows: rows.collect(),
        }
    }

    #[test]
    fn no_key_of_the_app_table_is_held_by_two_commands() {
        let map = Map::new(&Commands::new(declared()), &Settings::default(), any);
        assert_eq!(map.problems, []);
    }

    #[test]
    fn two_commands_cannot_share_a_chord_even_when_aimed_at_two_kinds() {
        static SAME: [Command; 2] = [
            row("a.one", "cmd+alt+1", &[]),
            row("b.two", "alt+cmd+1", &[]),
        ];
        let map = Map::new(&table(&SAME), &Settings::default(), any);
        assert_eq!(map.command("alt+cmd+1"), Some(CommandId("a.one")));
        let taken = Problem::Taken {
            key: "alt+cmd+1".to_owned(),
            kept: CommandId("a.one"),
            refused: CommandId("b.two"),
        };
        assert_eq!(map.problems, [taken]);
        static APART: [Command; 2] = [
            row("a.one", "return", &[Kind::Approval]),
            row("b.two", "return", &[Kind::Thread]),
        ];
        let map = Map::new(&table(&APART), &Settings::default(), any);
        assert_eq!(map.command("return"), Some(CommandId("a.one")));
        assert_eq!(
            map.key(CommandId("b.two")),
            None,
            "the key runs one command"
        );
    }

    #[test]
    fn the_users_key_replaces_the_tables_and_is_marked() -> Result<(), Box<dyn Error>> {
        static ONE: [Command; 1] = [row("view.fold", "alt+cmd+s", &[])];
        let set = user(&[("view.fold", "cmd+shift+b")]);
        let map = Map::new(&table(&ONE), &set, any);
        let found = map.rows().first().ok_or("no row")?;
        assert_eq!(found.key.as_deref(), Some("cmd+shift+b"));
        assert!(found.changed);
        assert_eq!(map.hint(CommandId("view.fold")).as_deref(), Some("⌘⇧B"));
        assert_eq!(map.command("alt+cmd+s"), None, "the old key runs nothing");
        let same = user(&[("view.fold", "cmd+alt+s")]);
        let map = Map::new(&table(&ONE), &same, any);
        assert!(
            !map.rows().iter().any(|row| row.changed),
            "the same key is no change"
        );
        Ok(())
    }

    #[test]
    fn none_unbinds_and_a_bad_chord_is_reported_with_the_default_kept() {
        static ONE: [Command; 1] = [row("view.fold", "alt+cmd+s", &[])];
        let gone = Map::new(&table(&ONE), &user(&[("view.fold", "None")]), any);
        assert_eq!(gone.key(CommandId("view.fold")), None);
        assert_eq!(gone.problems, []);
        let bad = Map::new(&table(&ONE), &user(&[("view.fold", "cmd+bogus")]), |text| {
            text != "cmd+bogus"
        });
        assert_eq!(bad.key(CommandId("view.fold")), Some("alt+cmd+s"));
        let said = bad.problems.first().map(ToString::to_string);
        assert_eq!(
            said.as_deref(),
            Some(
                r#"keys.view.fold: "cmd+bogus" is not a key, such as cmd+shift+k; the default stays"#
            )
        );
    }

    #[test]
    fn a_user_key_that_takes_anothers_goes_without_and_costs_no_default() {
        static TWO: [Command; 2] = [row("a.one", "cmd+k", &[]), row("b.two", "cmd+j", &[])];
        let set = user(&[("a.one", "cmd+j")]);
        let map = Map::new(&table(&TWO), &set, any);
        assert_eq!(map.command("cmd+j"), Some(CommandId("b.two")));
        assert_eq!(map.key(CommandId("a.one")), None);
        assert_eq!(map.problems.len(), 1);
    }

    #[test]
    fn the_groups_follow_the_mac_menus_and_list_only_keyed_commands() {
        let map = Map::new(&Commands::new(declared()), &Settings::default(), any);
        let names: Vec<&str> = map.groups().iter().map(|(name, _)| *name).collect();
        assert_eq!(
            names,
            ["Yi", "File", "View", "Thread", "Window", "Help", OTHER]
        );
        let mut lists = map.groups().into_iter().flat_map(|(_, rows)| rows);
        assert!(lists.all(|row| row.key.is_some()));
    }

    #[test]
    fn the_keys_group_has_a_row_for_each_command_with_the_tables_key_as_its_default() {
        let table = Commands::new(declared());
        let rows = settings(&table);
        let open = rows
            .iter()
            .find(|row| row.key.as_str() == "keys.command.open");
        assert_eq!(
            open.and_then(|row| row.default.clone()),
            Some(SettingValue::Text("cmd+k".into()))
        );
        let ids: BTreeSet<&str> = rows.iter().map(|row| row.key.as_str()).collect();
        assert_eq!(ids.len(), rows.len(), "one row for a command");
    }
}
