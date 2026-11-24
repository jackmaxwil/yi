//! A finding and the row the lists draw for it, both ways: the model's findings become rows,
//! and rows a feed or a scene sends become findings for the model's counts and gate.

use yd_core::address::uri::Address;
use yd_core::findings::state::{Mark, Stage};
use yd_core::findings::triage::FindingId;
use yd_core::findings::view::{Check, Finding, Origin, Ready};
use yd_core::ids::{Millis, ThreadId};
use yd_core::kind::Kind;
use yd_core::row::{Row, Value};

use crate::findings::desc::{
    mark_of, mark_words, severity_badge, severity_of, stage_of, stage_pill,
};

/// The meta-line mark of a finding that stands in the way of calling the work done.
pub const BLOCKS: &str = "blocks done";

const MINUTE: u64 = 60_000;

/// `now`, `12m`, `3h`, `2d`: how long ago, in the list's own short form.
pub fn age(ago: Millis) -> String {
    let minutes = ago.0 / MINUTE;
    match minutes {
        0 => "now".to_owned(),
        1..60 => format!("{minutes}m"),
        60..1440 => format!("{}h", minutes / 60),
        _ => format!("{}d", minutes / 1440),
    }
}

/// What a place is called on screen: a file's name or path, else the address's own id.
pub fn place_label(place: &Address) -> String {
    place.id().to_owned()
}

fn origin(origin: Origin) -> &'static str {
    match origin {
        Origin::Todo => "agent · todo",
        Origin::Discovery => "agent · goal",
        Origin::Claim => "agent · claim",
        Origin::You => "you",
    }
}

/// The buttons each state offers (record 6.7), then Attach and Dismiss.
fn actions(finding: &Finding) -> Vec<String> {
    let waiting = finding
        .checks
        .iter()
        .any(|check| check.mark == Mark::Waiting);
    let own: &[&str] = match finding.stage {
        Stage::New => &["Accept", "Not a problem", "Merge"],
        Stage::Open => &["Give it to an agent", "Ask for proof"],
        Stage::Fixing => &["Ask for proof", "Open its agent"],
        Stage::Proving if waiting => &["Allow the test ↵", "Open its agent"],
        Stage::Proving => &["Ask for proof", "Open its agent"],
        Stage::Watching => &["Give it to an agent"],
        Stage::Done => &[],
    };
    let tail = ["Attach", "Dismiss"];
    own.iter()
        .chain(tail.iter())
        .map(|&said| said.to_owned())
        .collect()
}

fn tags(words: impl IntoIterator<Item = String>) -> Value {
    Value::Tags(words.into_iter().collect())
}

/// The row the model's finding draws as. `gate` says which findings block.
pub fn row_of(finding: &Finding, gate: &Ready) -> Row {
    let (severity, level) = severity_badge(finding.severity);
    let (state, role) = stage_pill(finding.stage);
    let place = place_label(&finding.place);
    let owner = finding
        .owner
        .as_ref()
        .map(|agent| agent.as_str().to_owned());
    let who = match (&owner, finding.origin) {
        (Some(owner), _) => owner.clone(),
        (None, Origin::You) => "you".to_owned(),
        (None, _) => "nobody yet".to_owned(),
    };
    let blocks = gate.high.contains(&finding.id);
    let meta = [
        Some(place.clone()),
        Some(origin(finding.origin).to_owned()),
        owner,
    ]
    .into_iter()
    .flatten()
    .chain(blocks.then(|| BLOCKS.to_owned()));
    let mut row = Row::new(finding.id.as_str())
        .cell("severity", Value::Badge(severity.to_owned(), level))
        .text("finding", finding.title.as_str())
        .cell("state", Value::Pill(state.to_owned(), role))
        .text("where", place)
        .text("who_has_it", who)
        .cell("meta", tags(meta))
        .cell("actions", tags(actions(finding)));
    if let Some(ago) = finding.age {
        row = row.text("age", age(ago));
    }
    if blocks {
        row = row.text("blocks", BLOCKS);
    }
    if let Some(first) = finding.checks.first() {
        row = row
            .text("proof", first.text.as_str())
            .text("proof_state", mark_words(first.mark).1);
    }
    if !finding.checks.is_empty() {
        let each = |pick: fn(&Check) -> String| tags(finding.checks.iter().map(pick));
        row = row
            .cell("checks", each(|check| check.text.clone()))
            .cell(
                "check_notes",
                each(|check| mark_words(check.mark).1.to_owned()),
            )
            .cell(
                "check_marks",
                each(|check| mark_words(check.mark).0.to_owned()),
            );
    }
    if !finding.evidence.is_empty() {
        let labels = finding.evidence.iter().map(|thing| thing.label.clone());
        row = row.cell("evidence", tags(labels));
    }
    row
}

/// The place a row names: its first meta item, else where it is, without a line number.
fn place_of(row: &Row) -> Option<Address> {
    let first = match row.get("meta") {
        Some(Value::Tags(meta)) => meta.first().map(String::as_str),
        _ => None,
    };
    let said = first.or_else(|| row.words("where"))?;
    let said = said.split(" · ").next().unwrap_or(said);
    let file = said
        .rsplit_once(':')
        .filter(|(name, line)| name.contains('.') && line.parse::<u32>().is_ok())
        .map_or(said, |(name, _)| name);
    Address::new(Kind::Place, file).ok()
}

/// A row as a finding. A row with no state pill is a suggestion, as the triage box draws it.
pub fn finding_of(row: &Row, thread: &ThreadId) -> Option<Finding> {
    let pill = match row.get("state") {
        Some(Value::Pill(words, _)) => Some(words.as_str()),
        _ => None,
    };
    let stage = pill.map_or(Some(Stage::New), stage_of)?;
    let severity = match row.get("severity") {
        Some(Value::Badge(words, _)) => severity_of(words)?,
        _ => return None,
    };
    let marks = match row.get("check_marks") {
        Some(Value::Tags(marks)) => marks.iter().map(|mark| mark_of(mark)).collect(),
        _ => {
            let waiting = row.words("proof_state") == Some(mark_words(Mark::Waiting).1);
            let mark = if waiting {
                Mark::Waiting
            } else {
                Mark::Pending
            };
            row.words("proof")
                .map(|_| mark)
                .into_iter()
                .collect::<Vec<_>>()
        }
    };
    let proof = row.words("proof").unwrap_or_default();
    Some(Finding {
        id: FindingId::new(row.id.as_str()),
        thread: thread.clone(),
        title: row.words("finding")?.to_owned(),
        severity,
        stage,
        origin: Origin::Todo,
        place: place_of(row)?,
        owner: None,
        age: None,
        evidence: Vec::new(),
        checks: marks
            .into_iter()
            .map(|mark| Check {
                text: proof.to_owned(),
                mark,
            })
            .collect(),
    })
}
