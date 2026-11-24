//! The Daemons list: columns and what an opened service shows.

use yd_core::address::attach::ATTACH;
use yd_core::command::table::{Command, Route};
use yd_core::command::tools::service::{LOGS, RESTART, START, STOP};
use yd_core::feed::DAEMONS;
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

use crate::copy::labels;

const fn button(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Service],
        route: Route::Source(DAEMONS),
    }
}

/// Signing a service in to what it talks to. Nothing carries it out: a press says so.
pub const SIGN_IN: CommandId = CommandId("service.sign-in");

/// The buttons of an opened service, by the words its row's `actions` cell names them with.
/// The window carries each out (`feed::carry`); the table's own rows are elsewhere.
const COMMANDS: &[Command] = &[
    button(START, "Start"),
    button(STOP, "Stop"),
    button(RESTART, "Restart"),
    button(LOGS, "Logs"),
    button(SIGN_IN, "Sign in"),
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
    kind: Kind::Service,
    columns: &[
        col("state", "", Width::Px(10.0), Text::Plain),
        col("name", "Name", Width::Fr(1.0), Text::Name),
        col("what_it_is", "What it is", Width::Px(148.0), Text::Plain),
        col("pid", "PID", Width::Px(44.0), Text::Number),
        col("up", "Up", Width::Px(46.0), Text::Number),
        col("restarts", "Restarts", Width::Px(42.0), Text::Number),
        col("login", "Login", Width::Px(34.0), Text::Plain),
    ],
    // The lines `service.logs` reads hang under their service, each in its `name` cell.
    tree: true,
    height: RowHeight::Row,
    facts_per_line: 3,
    detail,
    footer: None,
};

const FACTS: [(&str, &str); 6] = [
    ("What", "what_it_is"),
    ("PID", "pid"),
    ("Up", "up"),
    ("Restarts", "restarts"),
    ("At login", "at_login"),
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
        .chain([(ATTACH, labels::ATTACH.to_owned())])
        .collect();
    Detail {
        facts: FACTS.iter().filter_map(fact).collect(),
        commands: buttons.iter().map(|(id, _)| *id).collect(),
        titles: buttons,
        note: row.words("note").map(str::to_owned),
        ..Detail::default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::rows_from_json;

    use super::*;

    const ROWS: &str = include_str!("../../../../fixtures/mockup/daemons.rows.json");

    fn open(id: &str) -> Result<Detail, Box<dyn Error>> {
        let rows = rows_from_json(ROWS)?;
        let row = rows
            .iter()
            .find(|row| row.id.as_str() == id)
            .ok_or("no such service")?;
        Ok(detail(row))
    }

    #[test]
    fn a_failed_service_leads_with_the_fix_not_a_restart() -> Result<(), Box<dyn Error>> {
        let failed = open("mcp-figma")?;
        assert_eq!(failed.commands, [SIGN_IN, RESTART, LOGS, ATTACH]);
        assert_eq!(failed.titles[0].1, "Sign in");
        assert_eq!(failed.note.as_deref(), Some("down for 56 minutes"));
        Ok(())
    }

    #[test]
    fn a_stopped_service_offers_start_and_no_stop() -> Result<(), Box<dyn Error>> {
        assert_eq!(open("forge-runner")?.commands, [START, LOGS, ATTACH]);
        Ok(())
    }
}
