//! The VMs list: columns and what an opened machine shows.

use yd_core::address::attach::ATTACH;
use yd_core::command::table::{Command, Route};
use yd_core::feed::VMS;
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

pub const OPEN_SCREEN: CommandId = CommandId("vm.open-screen");
pub const SNAPSHOT: CommandId = CommandId("vm.snapshot");
pub const RESTART: CommandId = CommandId("vm.restart");
pub const STOP: CommandId = CommandId("vm.stop");
pub const START: CommandId = CommandId("vm.start");
pub const RESUME: CommandId = CommandId("vm.resume");
pub const DELETE: CommandId = CommandId("vm.delete");

const fn button(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Vm],
        route: Route::Source(VMS),
    }
}

/// The buttons of an opened VM, with the words its row's `actions` cell names them by.
pub const COMMANDS: &[Command] = &[
    button(OPEN_SCREEN, "Open screen"),
    button(SNAPSHOT, "Snapshot"),
    button(RESTART, "Restart"),
    button(STOP, "Stop"),
    button(START, "Start"),
    button(RESUME, "Resume"),
    button(DELETE, "Delete"),
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
    kind: Kind::Vm,
    columns: &[
        col("state", "", Width::Px(10.0), Text::Plain),
        col("name", "Name", Width::Fr(1.0), Text::Name),
        col("cpu", "CPU", Width::Px(30.0), Text::Number),
        col("memory", "Memory", Width::Px(128.0), Text::Plain),
        col("disk", "Disk", Width::Px(66.0), Text::Plain),
        col("address", "Address", Width::Px(86.0), Text::Mono),
        col("snaps", "Snaps", Width::Px(34.0), Text::Number),
        col("up", "Up", Width::Px(40.0), Text::Number),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 3,
    detail,
    footer: None,
};

const FACTS: [(&str, &str); 6] = [
    ("Host", "host"),
    ("System", "system"),
    ("Memory", "memory_detail"),
    ("Disk", "disk"),
    ("Address", "address"),
    ("Snapshots", "snapshots"),
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
        ..Detail::default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::rows_from_json;

    use super::*;

    const ROWS: &str = include_str!("../../../../fixtures/mockup/vms.rows.json");

    fn open(id: &str) -> Result<Detail, Box<dyn Error>> {
        let rows = rows_from_json(ROWS)?;
        let row = rows
            .iter()
            .find(|row| row.id.as_str() == id)
            .ok_or("no such VM")?;
        Ok(detail(row))
    }

    #[test]
    fn a_running_vm_offers_its_screen_and_a_stopped_one_starts() -> Result<(), Box<dyn Error>> {
        assert_eq!(
            open("ci-ubuntu")?.commands,
            [OPEN_SCREEN, SNAPSHOT, RESTART, STOP, ATTACH]
        );
        assert_eq!(
            open("runner-b")?.commands,
            [START, SNAPSHOT, DELETE, ATTACH]
        );
        Ok(())
    }

    #[test]
    fn a_stopped_vm_with_no_memory_in_use_says_so() -> Result<(), Box<dyn Error>> {
        let shown = open("runner-b")?;
        assert_eq!(
            shown.facts[2].1,
            Value::Text("16 GB, not in use".to_owned())
        );
        Ok(())
    }
}
