//! The Findings list: columns, the words each button goes by, and what an opened finding shows.

use yd_core::address::attach::ATTACH;
use yd_core::command::by_kind::{FINDING_ASSIGN, FINDING_DISMISS};
use yd_core::command::table::{Command, Route};
use yd_core::findings::state::{Mark, Stage};
use yd_core::findings::view::Severity;
use yd_core::ids::CommandId;
use yd_core::kind::Kind;
use yd_core::list::{Col, Detail, ListDesc, Main, RowHeight, Text, Width};
use yd_core::row::{Row, Tone, Value};

pub use yd_core::command::by_kind::{
    FINDING_ACCEPT as ACCEPT, FINDING_ALLOW_TEST as ALLOW_TEST, FINDING_MERGE as MERGE,
    FINDING_NOT_A_PROBLEM as NOT_A_PROBLEM, FINDING_PROOF as ASK_PROOF,
};

pub const FIX_NOW: CommandId = CommandId("finding.fix-now");
pub const OPEN_AGENT: CommandId = CommandId("finding.open-agent");
pub const STOP_WATCHING: CommandId = CommandId("finding.stop-watching");
pub const SEND_TO_TRACKER: CommandId = CommandId("finding.send-to-tracker");
pub const ACCEPT_ALL: CommandId = CommandId("findings.accept-all");
/// Turns the pane into the hub, the page work runs from; the hub's header goes back.
pub const HUB: CommandId = CommandId("findings.hub");

const fn aimed(id: CommandId, title: &'static str) -> Command {
    Command {
        id,
        title,
        key: None,
        menu: None,
        kinds: &[Kind::Finding],
        route: Route::Model,
    }
}

/// The pane's own commands; the triage answers, attach and `finding.new` are core's rows,
/// since the right-click menu lists them.
pub const COMMANDS: &[Command] = &[
    aimed(FIX_NOW, "Fix Now"),
    aimed(OPEN_AGENT, "Open Its Agent"),
    aimed(STOP_WATCHING, "Stop Watching"),
    aimed(SEND_TO_TRACKER, "Send to Tracker"),
    Command {
        kinds: &[],
        ..aimed(ACCEPT_ALL, "Accept All Findings")
    },
    Command {
        kinds: &[],
        ..aimed(HUB, "Open Findings Hub")
    },
];

/// The words a button is drawn with and the command each runs (record 5.6, 6.6.2).
const WORDS: &[(&str, CommandId)] = &[
    ("Accept", ACCEPT),
    ("Not a problem", NOT_A_PROBLEM),
    ("Merge", MERGE),
    ("Give it to an agent", FINDING_ASSIGN),
    ("Reassign", FINDING_ASSIGN),
    ("Fix now", FIX_NOW),
    ("Ask for proof", ASK_PROOF),
    ("Allow the test", ALLOW_TEST),
    ("Open its agent", OPEN_AGENT),
    ("Stop watching", STOP_WATCHING),
    ("Send to tracker", SEND_TO_TRACKER),
    ("Attach", ATTACH),
    ("Dismiss", FINDING_DISMISS),
];

/// The command a button's words name; the keycap after them is the mockup's drawing of it.
pub fn command_of(words: &str) -> Option<CommandId> {
    let words = words.trim_end_matches('↵').trim_end();
    WORDS
        .iter()
        .find(|(said, _)| *said == words)
        .map(|(_, id)| *id)
}

/// The buttons a row's `actions` cell names: command, words, and whether it shows Return.
pub fn buttons(row: &Row) -> Vec<(CommandId, String, bool)> {
    let names: &[String] = match row.get("actions") {
        Some(Value::Tags(names)) => names,
        _ => &[],
    };
    let named = |name: &String| {
        let id = command_of(name)?;
        let words = name.trim_end_matches('↵').trim_end().to_owned();
        Some((id, words, name.ends_with('↵')))
    };
    names.iter().filter_map(named).collect()
}

/// What a state is called and the pill role it draws in (record 5.6).
pub fn stage_pill(stage: Stage) -> (&'static str, Tone) {
    match stage {
        Stage::New => ("New", Tone::Context),
        Stage::Open => ("Open", Tone::Dim),
        Stage::Fixing => ("Fixing", Tone::Accent),
        Stage::Proving => ("Proving", Tone::Need),
        Stage::Watching => ("Watching", Tone::Ok),
        Stage::Done => ("Done", Tone::Ok),
    }
}

pub fn stage_of(words: &str) -> Option<Stage> {
    Stage::ALL
        .into_iter()
        .find(|stage| stage_pill(*stage).0 == words)
}

pub fn severity_badge(severity: Severity) -> (&'static str, Tone) {
    match severity {
        Severity::High => ("High", Tone::Bad),
        Severity::Med => ("Med", Tone::Warn),
        Severity::Low => ("Low", Tone::Dim),
    }
}

pub fn severity_of(words: &str) -> Option<Severity> {
    [Severity::High, Severity::Med, Severity::Low]
        .into_iter()
        .find(|severity| severity_badge(*severity).0 == words)
}

/// A check's mark as the record writes it (`y`, `n`, `w`, `pe`) and the words beside it.
pub fn mark_words(mark: Mark) -> (&'static str, &'static str) {
    match mark {
        Mark::Met => ("y", "passed"),
        Mark::Failing => ("n", "failing"),
        Mark::Waiting => ("w", "waiting on you"),
        Mark::Pending => ("pe", "not started"),
    }
}

pub fn mark_of(letter: &str) -> Mark {
    match letter {
        "y" => Mark::Met,
        "n" => Mark::Failing,
        "w" => Mark::Waiting,
        _ => Mark::Pending,
    }
}

const fn col(field: &'static str, title: &'static str, width: Width, text: Text) -> Col {
    Col {
        field,
        title,
        width,
        text,
    }
}

/// The compact list (record 5.6): grid `32px 1fr 104px 58px 86px 24px`.
pub const DESC: ListDesc = ListDesc {
    kind: Kind::Finding,
    columns: &[
        col("severity", "", Width::Px(32.0), Text::Plain),
        col("finding", "Finding", Width::Fr(1.0), Text::Name),
        col("where", "Where", Width::Px(104.0), Text::Mono),
        col("state", "State", Width::Px(58.0), Text::Plain),
        col("who_has_it", "Who has it", Width::Px(86.0), Text::Plain),
        col("age", "Age", Width::Px(24.0), Text::Number),
    ],
    tree: false,
    height: RowHeight::Row,
    facts_per_line: 1,
    detail,
    footer: None,
};

/// Why, the proof with its status, the evidence; then the buttons the row names.
fn detail(row: &Row) -> Detail {
    let mut facts = Vec::new();
    if let Some(why) = row.get("why") {
        facts.push(("Why".to_owned(), why.clone()));
    }
    if let Some(proof) = row.words("proof") {
        let said = match row.words("proof_state") {
            Some(state) => format!("{proof} · {state}"),
            None => proof.to_owned(),
        };
        facts.push(("Proof".to_owned(), Value::Text(said)));
    }
    if let Some(evidence) = row.get("evidence") {
        facts.push(("Evidence".to_owned(), evidence.clone()));
    }
    let shown = buttons(row);
    Detail {
        facts,
        commands: shown.iter().map(|(id, _, _)| *id).collect(),
        titles: shown
            .iter()
            .map(|(id, words, _)| (*id, words.clone()))
            .collect(),
        hints: shown
            .iter()
            .filter(|(_, _, keyed)| *keyed)
            .map(|(id, _, _)| (*id, "return"))
            .collect(),
        quiet: vec![FINDING_DISMISS],
        main: Main::First,
        ..Detail::default()
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_core::row::rows_from_json;

    use super::*;

    const ROWS: &str = include_str!("../../../../fixtures/mockup/findings-table.rows.json");

    #[test]
    fn every_button_the_mockup_draws_names_a_command() -> Result<(), Box<dyn Error>> {
        for row in rows_from_json(ROWS)? {
            let Some(Value::Tags(names)) = row.get("actions") else {
                continue;
            };
            assert_eq!(buttons(&row).len(), names.len(), "{}", row.id.as_str());
        }
        Ok(())
    }

    #[test]
    fn the_proving_finding_opens_with_its_test_as_the_main_button() -> Result<(), Box<dyn Error>> {
        let rows = rows_from_json(ROWS)?;
        let open = rows.first().ok_or("no rows")?;
        let shown = detail(open);
        assert_eq!(
            shown.commands,
            [ALLOW_TEST, OPEN_AGENT, ATTACH, FINDING_DISMISS]
        );
        assert_eq!(shown.hints, [(ALLOW_TEST, "return")]);
        let labels: Vec<&str> = shown.facts.iter().map(|(l, _)| l.as_str()).collect();
        assert_eq!(labels, ["Why", "Proof", "Evidence"]);
        Ok(())
    }
}
