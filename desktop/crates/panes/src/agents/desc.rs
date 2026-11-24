//! The Agents list: columns and what an opened agent shows.

use yd_core::command::table::{Command, Route};
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, Main, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

pub const RUN_COMMAND: CommandId = CommandId("agents.run-command");
pub const OPEN_THREAD: CommandId = CommandId("agents.open-thread");
pub const PAUSE: CommandId = CommandId("agents.pause");
pub const MORE_BUDGET: CommandId = CommandId("agents.more-budget");
pub const STOP: CommandId = CommandId("agents.stop");

const fn button(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Agent],
        route: Route::Model,
    }
}

/// The five buttons of an opened agent, with the words the row's `actions` cell names them by.
pub const COMMANDS: &[Command] = &[
    button(RUN_COMMAND, "Run its command"),
    button(OPEN_THREAD, "Open thread"),
    button(PAUSE, "Pause"),
    button(MORE_BUDGET, "More budget"),
    button(STOP, "Stop"),
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
    kind: Kind::Agent,
    columns: &[
        col("state", "", Width::Px(7.0), Text::Plain),
        col("agent", "Agent", Width::Px(131.0), Text::Name),
        col("doing_now", "Doing now", Width::Fr(1.0), Text::Plain),
        col("model", "", Width::Px(12.0), Text::Mono),
        col("tokens", "Tokens", Width::Px(38.0), Text::Number),
        col("cost", "Cost", Width::Px(36.0), Text::Number),
        col("time", "Time", Width::Px(30.0), Text::Number),
        col("last_14_min", "Last 14 min", Width::Px(60.0), Text::Plain),
    ],
    tree: true,
    height: RowHeight::Row,
    facts_per_line: 1,
    detail,
    footer: None,
};

const FACTS: [(&str, &str); 4] = [
    ("Asked to", "asked_to"),
    ("Said last", "said_last"),
    ("Touched", "touched"),
    ("Budget", "budget"),
];

/// The command a button's words name; the keycap after them is the mockup's drawing of it.
fn command_of(title: &str) -> Option<CommandId> {
    let title = title.trim_end_matches('↵').trim_end();
    COMMANDS
        .iter()
        .find(|command| command.title == title)
        .map(|command| command.id)
}

/// Facts in the order the record gives them, buttons as the row's `actions` name them.
fn detail(row: &Row) -> Detail {
    let fact = |(label, field): &(&str, &str)| Some(((*label).to_owned(), row.get(field)?.clone()));
    let names: &[String] = match row.get("actions") {
        Some(Value::Tags(names)) => names,
        _ => &[],
    };
    let commands: Vec<CommandId> = names.iter().filter_map(|name| command_of(name)).collect();
    // A button the row writes with the Return keycap shows it; the key itself is the card's.
    let keyed = names.iter().filter(|name| name.ends_with('↵'));
    let hints = keyed.filter_map(|name| Some((command_of(name)?, "return")));
    let main = if commands.contains(&RUN_COMMAND) {
        Main::Command(RUN_COMMAND)
    } else {
        Main::Neither
    };
    Detail {
        facts: FACTS.iter().filter_map(fact).collect(),
        quiet: commands.iter().copied().filter(|id| *id == STOP).collect(),
        hints: hints.collect(),
        commands,
        note: row.words("note").map(str::to_owned),
        main,
        ..Detail::default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::rows_from_json;

    use super::*;

    const ROWS: &str = include_str!("../../../../fixtures/mockup/agents.rows.json");

    #[test]
    fn the_open_agent_shows_the_record_facts_and_buttons() -> Result<(), Box<dyn Error>> {
        let rows = rows_from_json(ROWS)?;
        let open = rows
            .iter()
            .find(|row| row.id.as_str() == "tests-upload")
            .ok_or("no open row")?;
        let shown = detail(open);
        let labels: Vec<&str> = shown
            .facts
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(labels, ["Asked to", "Said last", "Touched", "Budget"]);
        assert_eq!(
            shown.commands,
            [RUN_COMMAND, OPEN_THREAD, PAUSE, MORE_BUDGET, STOP]
        );
        assert_eq!(shown.main, Main::Command(RUN_COMMAND));
        assert_eq!(shown.quiet, [STOP]);
        Ok(())
    }

    #[test]
    fn a_row_whose_buttons_lack_the_main_one_has_no_filled_button() {
        let row = Row::new("done").cell("actions", Value::Tags(vec!["Open thread".to_owned()]));
        let shown = detail(&row);
        assert_eq!(shown.commands, [OPEN_THREAD]);
        assert_eq!(shown.main, Main::Neither);
    }
}
