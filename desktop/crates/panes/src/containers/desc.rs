//! The Containers list: columns and what an opened container shows.

use yd_core::address::attach::ATTACH;
use yd_core::command::table::{Command, Route};
use yd_core::command::tools::container::{LOGS, REMOVE, RESTART, SHELL, START, STOP};
use yd_core::feed::CONTAINERS;
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

const fn button(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Container],
        route: Route::Source(CONTAINERS),
    }
}

/// The buttons of an opened container, by the words its row's `actions` cell names them
/// with. The window carries each out (`feed::carry`); the table's own rows are `yd-core`'s.
const COMMANDS: &[Command] = &[
    button(LOGS, "Logs"),
    button(SHELL, "Shell"),
    button(RESTART, "Restart"),
    button(STOP, "Stop"),
    button(START, "Start"),
    button(REMOVE, "Remove"),
];

const fn col(field: &'static str, title: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title,
        width,
        text,
    }
}

pub const DESC: ListDesc = ListDesc {
    kind: Kind::Container,
    columns: &[
        col("state", "", Width::Px(10.0), Text::Plain),
        col("name", "Name", Width::Px(88.0), Text::Name),
        col("image", "Image", Width::Fr(1.0), Text::Mono),
        col("cpu", "CPU", Width::Px(68.0), Text::Plain),
        col("mem", "Mem", Width::Px(50.0), Text::Number),
        col("ports", "Ports", Width::Px(60.0), Text::Mono),
        col("up", "Up", Width::Px(34.0), Text::Number),
        // The four row action icons have no cell in the fixture; the column keeps its place.
        col("", "", Width::Px(50.0), Text::Plain),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 3,
    detail,
    footer: None,
};

const FACTS: [(&str, &str); 6] = [
    ("Image", "image"),
    ("Ports", "ports"),
    ("Health", "health"),
    ("Memory", "memory"),
    ("Restarts", "restarts"),
    ("Last line", "last_line"),
];

fn command_of(title: &str) -> Option<CommandId> {
    COMMANDS
        .iter()
        .find(|command| command.title == title)
        .map(|command| command.id)
}

/// The facts, then the row's buttons as its `actions` names them, then Attach, which every row ends with.
/// Each button keeps the words its row gives it: a global row may title the same command differently.
fn detail(row: &Row) -> Detail {
    let fact = |(label, field): &(&str, &str)| Some(((*label).to_owned(), row.get(field)?.clone()));
    let names: &[String] = match row.get("actions") {
        Some(Value::Tags(names)) => names,
        _ => &[],
    };
    let buttons: Vec<(CommandId, String)> = names
        .iter()
        .filter_map(|name| Some((command_of(name)?, name.clone())))
        .chain([(ATTACH, "Attach".to_owned())])
        .collect();
    Detail {
        facts: FACTS.iter().filter_map(fact).collect(),
        commands: buttons.iter().map(|(id, _)| *id).collect(),
        titles: buttons,
        note: row.words("note").map(str::to_owned),
        text: row.words("output").map(str::to_owned),
        ..Detail::default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::rows_from_json;

    use super::*;

    const ROWS: &str = include_str!("../../../../fixtures/mockup/containers.rows.json");

    fn open(id: &str) -> Result<Detail, Box<dyn Error>> {
        let rows = rows_from_json(ROWS)?;
        let row = rows
            .iter()
            .find(|row| row.id.as_str() == id)
            .ok_or("no such container")?;
        Ok(detail(row))
    }

    #[test]
    fn a_running_container_leads_with_its_logs_and_an_exited_one_with_start()
    -> Result<(), Box<dyn Error>> {
        assert_eq!(
            open("atlas-api")?.commands,
            [LOGS, SHELL, RESTART, STOP, ATTACH]
        );
        assert_eq!(open("registry")?.commands, [START, LOGS, REMOVE, ATTACH]);
        Ok(())
    }

    #[test]
    fn the_open_container_shows_its_health_and_its_last_line() -> Result<(), Box<dyn Error>> {
        let shown = open("registry")?;
        let labels: Vec<&str> = shown
            .facts
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "Image",
                "Ports",
                "Health",
                "Memory",
                "Restarts",
                "Last line"
            ]
        );
        assert_eq!(
            shown.facts[2].1,
            Value::Text("stopped, out of memory".to_owned())
        );
        Ok(())
    }
}
