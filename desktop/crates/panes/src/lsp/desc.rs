//! The Language servers and Problems lists: columns and what an opened row shows.

use yd_core::address::attach::ATTACH;
use yd_core::address::uri::Address;
use yd_core::command::by_kind::ASK;
use yd_core::command::table::{Command, Route};
use yd_core::command::tools::lsp::{APPLY_FIX, FIXES, RESTART};
use yd_core::feed::LSP;
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, LabelWidth, ListDesc, Main, RowHeight, Text, Width};
use yd_core::row::{Row, Value};

pub use yd_core::command::by_kind::SERVER_LOGS as LOGS;

use crate::copy::labels;

/// Room for a server's longest label, "Agent may ask it", which 56 points cut.
const SERVER_LABEL: f32 = 78.0;
pub const SETTINGS: CommandId = CommandId("lsp.settings");
pub const IGNORE: CommandId = CommandId("lsp.ignore-here");
pub const OPEN_FINDING: CommandId = CommandId("lsp.open-finding");

const fn command(id: CommandId, title: &'static str, kind: Kind, route: Route) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: match kind {
            Kind::Problem => &[Kind::Problem],
            _ => &[Kind::LanguageServer],
        },
        route,
    }
}

/// The buttons of an opened server or problem that are this pane's own; restart and the log
/// are core's rows, since the right-click menu lists them. The first two reach the server.
pub const COMMANDS: &[Command] = &[
    command(
        APPLY_FIX,
        "Apply Quick Fix",
        Kind::Problem,
        Route::Source(LSP),
    ),
    command(
        FIXES,
        "Look for Quick Fixes",
        Kind::Problem,
        Route::Source(LSP),
    ),
    command(
        SETTINGS,
        "Language Server Settings",
        Kind::LanguageServer,
        Route::Model,
    ),
    command(IGNORE, "Ignore Problem Here", Kind::Problem, Route::Model),
    command(OPEN_FINDING, "Open Finding", Kind::Problem, Route::Model),
];

const fn col(field: &'static str, title: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title,
        width,
        text,
    }
}

/// Record 6.4.1: the grid `10px 1fr 60px 62px 52px`.
pub const SERVERS: ListDesc = ListDesc {
    kind: Kind::LanguageServer,
    columns: &[
        col("state", "", Width::Px(10.0), Text::Plain),
        col("server", "Server", Width::Fr(1.0), Text::Name),
        col("language", "Language", Width::Px(60.0), Text::Plain),
        col("project", "Project", Width::Px(62.0), Text::Mono),
        col("memory", "Memory", Width::Px(52.0), Text::Number),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 1,
    detail: server,
    footer: None,
};

/// Record 6.4.1: the grid `32px 1fr 112px`, with no column titles.
pub const PROBLEMS: ListDesc = ListDesc {
    kind: Kind::Problem,
    columns: &[
        col("severity", "", Width::Px(32.0), Text::Plain),
        col("message", "", Width::Fr(1.0), Text::Name),
        col("where", "", Width::Px(112.0), Text::Mono),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 1,
    detail: problem,
    footer: None,
};

/// What each word a row's `actions` cell offers does. A word the table lacks is not drawn.
const ACTIONS: [(&str, CommandId); 8] = [
    ("Restart", RESTART),
    ("Start now", RESTART),
    ("Logs", LOGS),
    ("Settings", SETTINGS),
    (labels::ATTACH, ATTACH),
    ("Ask agent", ASK),
    ("Open finding", OPEN_FINDING),
    ("Ignore here", IGNORE),
];

fn tags<'a>(row: &'a Row, field: &str) -> &'a [String] {
    match row.get(field) {
        Some(Value::Tags(all)) => all,
        _ => &[],
    }
}

fn number(row: &Row, field: &str) -> Option<usize> {
    row.words(field)?.parse().ok()
}

/// Where a problem is: the line and column its row names.
pub fn place(row: &Row) -> Option<Address> {
    let (path, line) = (row.words("path")?, number(row, "line")?);
    let col = number(row, "col").unwrap_or(1);
    let at = Address::new(Kind::Lines, path).ok()?;
    Some(at.part(format!("L{line}:C{col}")))
}

/// A problem's quick fix: applying the fix row the pane joined to it, else asking the
/// server for fixes at its place.
pub fn quick_fix(row: &Row) -> (CommandId, Option<Address>) {
    let fix = row.words("fix_row");
    match fix.and_then(|id| Address::new(Kind::Problem, id).ok()) {
        Some(fix) => (APPLY_FIX, Some(fix)),
        None => (FIXES, place(row)),
    }
}

/// The command a button word runs, aimed where it must be.
fn action(row: &Row, word: &str, detail: &mut Detail) {
    let (id, aim) = match word {
        "Quick fix" => quick_fix(row),
        _ => match ACTIONS.iter().find(|(name, _)| *name == word) {
            Some((_, id)) => (*id, None),
            None => return,
        },
    };
    detail.commands.push(id);
    detail.titles.push((id, word.to_owned()));
    detail.aims.extend(aim.map(|aim| (id, aim)));
}

fn buttons(row: &Row) -> Detail {
    let mut detail = Detail::default();
    for word in tags(row, "actions") {
        action(row, word, &mut detail);
    }
    detail
}

/// The state line, what the server can do, and whether the agent may ask it; none of the
/// buttons is the main one.
fn server(row: &Row) -> Detail {
    let fact = |label: &str, field: &str| Some((label.to_owned(), row.get(field)?.clone()));
    Detail {
        facts: [
            fact("Status", "status"),
            fact("Can do", "can_do"),
            fact("Agent may ask it", "agent_may_ask"),
        ]
        .into_iter()
        .flatten()
        .collect(),
        main: Main::Neither,
        label: LabelWidth(SERVER_LABEL),
        ..buttons(row)
    }
}

/// Why it matters, the fix and where it comes from, as far as the row says; the quick fix
/// is the main button and ignoring it here is quiet.
fn problem(row: &Row) -> Detail {
    let fact = |label: &str, field: &str| Some((label.to_owned(), row.get(field)?.clone()));
    Detail {
        facts: [fact("Why", "why"), fact("Fix", "fix"), fact("From", "from")]
            .into_iter()
            .flatten()
            .collect(),
        quiet: vec![IGNORE],
        ..buttons(row)
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::rows_from_json;

    use super::*;

    const SERVERS_ROWS: &str = include_str!("../../../../fixtures/mockup/lsp.rows.json");
    const PROBLEMS_ROWS: &str = include_str!("../../../../fixtures/mockup/problems.rows.json");

    fn titles(detail: &Detail) -> Vec<&str> {
        detail
            .titles
            .iter()
            .map(|(_, title)| title.as_str())
            .collect()
    }

    #[test]
    fn a_server_opens_to_its_state_its_abilities_and_the_buttons_the_record_lists()
    -> Result<(), Box<dyn Error>> {
        let rows = rows_from_json(SERVERS_ROWS)?;
        let [ready, _, off] = rows.as_slice() else {
            return Err("the fixture has three servers".into());
        };
        let detail = server(ready);
        let labels: Vec<&str> = detail
            .facts
            .iter()
            .map(|(label, _)| label.as_str())
            .collect();
        assert_eq!(labels, ["Status", "Can do", "Agent may ask it"]);
        assert_eq!(titles(&detail), ["Restart", "Logs", "Settings", "Attach"]);
        assert_eq!(detail.main, Main::Neither);
        assert_eq!(titles(&server(off)), ["Start now", "Settings", "Attach"]);
        Ok(())
    }

    #[test]
    fn a_problem_offers_the_buttons_its_row_names_and_a_finding_replaces_asking_the_agent()
    -> Result<(), Box<dyn Error>> {
        let rows = rows_from_json(PROBLEMS_ROWS)?;
        let details: Vec<Detail> = rows.iter().map(problem).collect();
        let named = |at: usize| details.get(at).map(titles);
        let ask = ["Quick fix", "Ask agent", "Attach", "Ignore here"];
        assert_eq!(named(0), Some(ask.to_vec()));
        assert_eq!(
            named(1),
            Some(vec!["Quick fix", "Open finding", "Attach", "Ignore here"])
        );
        Ok(())
    }

    #[test]
    fn a_quick_fix_applies_the_joined_fix_and_otherwise_asks_for_fixes_at_the_problem()
    -> Result<(), Box<dyn Error>> {
        let bare = Row::new("problem:src/a.rs:20:0")
            .text("path", "src/a.rs")
            .text("line", "20")
            .text("col", "25")
            .cell("actions", Value::Tags(vec!["Quick fix".to_owned()]));
        let asked = problem(&bare);
        assert_eq!(asked.commands, [FIXES]);
        let place = &asked.aims.first().ok_or("no aim")?.1;
        assert_eq!(place.part_text(), Some("L20:C25"));
        let joined = bare.text("fix_row", "fix:src/a.rs:20:0");
        let applied = problem(&joined);
        assert_eq!(applied.commands, [APPLY_FIX]);
        assert_eq!(
            applied.aims.first().map(|(_, at)| at.id()),
            Some("fix:src/a.rs:20:0")
        );
        Ok(())
    }
}
