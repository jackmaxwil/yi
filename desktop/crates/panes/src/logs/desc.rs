//! The Logs list: columns, level styling and what an opened line shows.

use yd_core::command::table::{Command, Route};
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, Main, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

use crate::copy::labels;

pub const SHOW_IN_TRACE: CommandId = CommandId("logs.show-in-trace");
pub const AROUND: CommandId = CommandId("logs.around");
pub const ONLY_SOURCE: CommandId = CommandId("logs.only-source");
pub const ATTACH: CommandId = CommandId("logs.attach");
pub const MAKE_FINDING: CommandId = CommandId("logs.make-finding");

const fn button(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::LogLine],
        route: Route::Model,
    }
}

/// The buttons of an opened line. The window carries them out, since each changes this
/// pane's own view or another pane: see [`super::pane::carry`].
pub const COMMANDS: &[Command] = &[
    button(SHOW_IN_TRACE, "Show in trace"),
    button(AROUND, "Logs around it"),
    button(ONLY_SOURCE, "Only this source"),
    button(ATTACH, labels::ATTACH),
    button(MAKE_FINDING, "Make it a finding"),
];

/// Record 5.5's grid on 16 px lines with no titles; the time is 80 wide, not 76, because the
/// system mono face needs 78 for twelve characters.
pub const DESC: ListDesc = ListDesc {
    kind: Kind::LogLine,
    columns: &[
        Col {
            field: "time",
            title: "",
            width: Width::Px(80.0),
            text: Text::Faint,
        },
        Col {
            field: "level",
            title: "",
            width: Width::Px(36.0),
            text: Text::Plain,
        },
        Col {
            field: "source",
            title: "",
            width: Width::Px(64.0),
            text: Text::Faint,
        },
        Col {
            field: "message",
            title: "",
            width: Width::Fr(1.0),
            text: Text::Plain,
        },
    ],
    tree: false,
    height: RowHeight::Line,
    facts_per_line: 3,
    detail,
    footer: None,
};

/// The context a line can carry, in the order the record lists them per source.
const CONTEXT: [(&str, &str); 15] = [
    ("Thread", "thread"),
    ("Agent", "agent"),
    ("Turn", "turn"),
    ("Process", "process"),
    ("Service", "service"),
    ("Container", "container"),
    ("Image", "image"),
    ("Host", "host"),
    ("Request", "request"),
    ("In trace", "in_trace"),
    ("App", "app"),
    ("Seen via", "seen_via"),
    ("Peer", "peer"),
    ("Pool", "pool"),
    ("Seen", "seen"),
];

/// Cells that are the line itself, not a fact about it; `span` is its step in the trace.
const OWN: [&str; 8] = [
    "ts", "time", "level", "source", "message", "json", "actions", "span",
];

fn title(field: &str) -> String {
    let words = field.replace('_', " ");
    let mut chars = words.chars();
    chars.next().map_or_else(String::new, |first| {
        first.to_uppercase().chain(chars).collect()
    })
}

/// Time, level and source first, then the context the record lists, then any field a parser
/// found; the raw record under them.
fn detail(row: &Row) -> Detail {
    let head = [("Time", "time"), ("Level", "level"), ("Source", "source")];
    let known = CONTEXT
        .iter()
        .map(|(label, field)| ((*label).to_owned(), *field));
    let listed = |field: &str| CONTEXT.iter().any(|(_, known)| *known == field);
    let rest = row
        .cells
        .keys()
        .filter(|field| !OWN.contains(&field.as_str()) && !listed(field))
        .map(|field| (title(field), field.as_str()));
    let facts = head
        .iter()
        .map(|(label, field)| ((*label).to_owned(), *field))
        .chain(known)
        .chain(rest)
        .filter_map(|(label, field)| {
            let text = row.words(field)?;
            Some((label, Value::Text(text.to_owned())))
        })
        .collect();
    Detail {
        facts,
        text: row
            .words("json")
            .or_else(|| row.words("message"))
            .map(str::to_owned),
        commands: COMMANDS.iter().map(|command| command.id).collect(),
        titles: row
            .words("source")
            .map(|source| (ONLY_SOURCE, format!("Only {source}")))
            .into_iter()
            .collect(),
        main: Main::Neither,
        ..Detail::default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::{Row, Value};

    use super::detail;

    #[test]
    fn a_line_opens_to_time_level_source_then_its_context_in_the_records_order()
    -> Result<(), Box<dyn Error>> {
        let row = Row::new("l")
            .text("time", "17:58:03.412")
            .cell(
                "level",
                Value::Badge("ERROR".into(), yd_core::row::Tone::Bad),
            )
            .text("source", "docker:db")
            .text("seen", "4 times this hour")
            .text("container", "atlas-db")
            .text("retry_after", "41")
            .text("host", "hv-01")
            .text("json", "{\"x\":1}");
        let shown = detail(&row);
        let labels: Vec<&str> = shown
            .facts
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(
            labels,
            [
                "Time",
                "Level",
                "Source",
                "Container",
                "Host",
                "Seen",
                "Retry after"
            ]
        );
        assert_eq!(shown.text.as_deref(), Some("{\"x\":1}"));
        assert_eq!(shown.commands.len(), 5);
        let bare = Row::new("m").text("message", "worker: nine sessions");
        assert_eq!(detail(&bare).text.as_deref(), Some("worker: nine sessions"));
        Ok(())
    }
}
