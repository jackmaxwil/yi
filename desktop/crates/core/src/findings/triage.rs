//! Triage the app keeps itself: what the user accepted, dismissed and merged, the findings the
//! user made, and the message each action sends the agent.

use std::collections::BTreeMap;
use std::fmt;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::address::thing::Thing;
use crate::address::uri::Address;
use crate::command::table::{Run, Target};
use crate::findings::state::Mark;
use crate::findings::view::{self, Finding, Scope};
use crate::ids::{Millis, ThreadId};
use crate::kind::Kind;
use crate::model::Model;
use crate::msg::Effect;
use crate::reduce::step::commands::{ALLOW, PROMPT};

/// The largest triage file read back; a bigger one is ignored whole.
pub const MAX_FILE: u64 = 4 << 20;
/// Decisions and made findings each, at most: past it a new one is not kept.
pub const MAX_ENTRIES: usize = 10_000;
/// The longest dismiss reason kept, in bytes; the prompt to the agent carries it whole.
pub const MAX_REASON: usize = 240;

/// A finding's stable id: `<thread>/todo/<todo>`, `<thread>/discovery/<fingerprint>`,
/// `<thread>/claim/<label>` or `made/<n>`.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FindingId(String);

impl FindingId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for FindingId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What the user said about a finding. Absent means the agent's suggestion stands untouched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "decision", rename_all = "snake_case")]
pub enum Decision {
    Accepted,
    Dismissed {
        reason: String,
    },
    /// A duplicate: its evidence shows under `into`, and it no longer shows itself.
    Merged {
        into: FindingId,
    },
}

/// A finding the user made from a thing with `finding.new`: a suggestion until accepted.
#[derive(Debug, Clone, PartialEq)]
pub struct Made {
    pub id: FindingId,
    /// The thread it was made in; none shows it in every thread.
    pub thread: Option<ThreadId>,
    pub evidence: Address,
    pub title: String,
    pub at: Millis,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct Triage {
    pub decisions: BTreeMap<FindingId, Decision>,
    pub made: Vec<Made>,
    next: u64,
    /// Rises on every change: the app saves when it moves past what it last wrote.
    pub revision: u64,
}

impl Triage {
    pub fn decision(&self, id: &FindingId) -> Option<&Decision> {
        self.decisions.get(id)
    }

    fn decide(&mut self, id: &FindingId, decision: Decision) {
        if !self.decisions.contains_key(id) && self.decisions.len() >= MAX_ENTRIES {
            return;
        }
        let decision = match decision {
            Decision::Dismissed { reason } => Decision::Dismissed {
                reason: clip(&reason),
            },
            other @ (Decision::Accepted | Decision::Merged { .. }) => other,
        };
        if self.decisions.get(id) != Some(&decision) {
            self.decisions.insert(id.clone(), decision);
            self.revision = self.revision.saturating_add(1);
        }
    }

    fn accept(&mut self, id: &FindingId) {
        if !self.decisions.contains_key(id) {
            self.decide(id, Decision::Accepted);
        }
    }
}

/// The longest start of `reason` that fits [`MAX_REASON`] bytes, cut between characters.
fn clip(reason: &str) -> String {
    let fits = |(at, c): &(usize, char)| at.saturating_add(c.len_utf8()) <= MAX_REASON;
    reason
        .char_indices()
        .take_while(fits)
        .map(|(_, c)| c)
        .collect()
}

/// What the user does to a finding. Each sends the agent one message and changes triage.
#[derive(Debug, Clone, PartialEq)]
pub enum Act {
    /// `thread.prompt` (`session/prompt`): the agent hears it is filed. Triage: accepted.
    Accept,
    /// `thread.prompt`: the agent is asked to fix it and prove it, by the agent named if any.
    Assign { agent: Option<String> },
    /// `thread.prompt`: the agent is asked to name and run the check that proves it.
    AskProof,
    /// `approval.allow` (an `allow_once` answer to `session/request_permission`) on the ask
    /// that runs its check; with no such ask, `thread.prompt` asking the agent to run it.
    AllowTest,
    /// `thread.prompt`: not a problem, with why, so the agent stops raising it.
    Dismiss { reason: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum TriageMsg {
    /// What the app's file held at start.
    Loaded(Triage),
    /// `finding.new`: a suggestion with this thing as its evidence.
    Make {
        thread: Option<ThreadId>,
        evidence: Address,
    },
    /// The user acts on a finding of `thread`'s project.
    Act {
        thread: ThreadId,
        finding: FindingId,
        act: Act,
    },
    /// The user says `finding` is the same as `into`. Merging is local; no message goes.
    Merge { finding: FindingId, into: FindingId },
}

pub fn step(model: &mut Model, change: TriageMsg) -> Vec<Effect> {
    match change {
        TriageMsg::Loaded(triage) => {
            model.findings.triage = triage;
            Vec::new()
        }
        TriageMsg::Make { thread, evidence } => {
            make(model, thread, evidence);
            Vec::new()
        }
        TriageMsg::Act {
            thread,
            finding,
            act,
        } => act_on(model, &thread, &finding, act),
        TriageMsg::Merge { finding, into } if finding != into => {
            model
                .findings
                .triage
                .decide(&finding, Decision::Merged { into });
            Vec::new()
        }
        TriageMsg::Merge { .. } => Vec::new(),
    }
}

/// Makes a suggestion whose evidence is `evidence`, titled by the thing's own name.
pub fn make(model: &mut Model, thread: Option<ThreadId>, evidence: Address) -> Option<FindingId> {
    if model.findings.triage.made.len() >= MAX_ENTRIES {
        return None;
    }
    let title = Thing::at(model, evidence.clone()).label;
    let triage = &mut model.findings.triage;
    let id = FindingId::new(format!("made/{}", triage.next));
    triage.next = triage.next.saturating_add(1);
    triage.made.push(Made {
        id: id.clone(),
        thread,
        evidence,
        title,
        at: model.now,
    });
    triage.revision = triage.revision.saturating_add(1);
    Some(id)
}

fn act_on(model: &mut Model, thread: &ThreadId, id: &FindingId, act: Act) -> Vec<Effect> {
    let shown = view::findings(model, thread, Scope::Project);
    let Some(finding) = shown.into_iter().find(|finding| &finding.id == id) else {
        return Vec::new();
    };
    let Ok(address) = finding.address() else {
        return Vec::new();
    };
    let on = &finding.thread;
    let title = &finding.title;
    let run = match &act {
        Act::Accept => prompt(
            on,
            format!("Filed finding {address}: {title}. Keep it until a check proves it fixed."),
        ),
        Act::Assign { agent } => {
            let by = agent
                .as_deref()
                .map(|agent| format!(" Give it to {agent}."))
                .unwrap_or_default();
            prompt(
                on,
                format!(
                    "Fix finding {address}: {title}. Prove the fix with a check before calling it done.{by}"
                ),
            )
        }
        Act::AskProof => prompt(
            on,
            format!("Name the check that proves finding {address} fixed, then run it: {title}."),
        ),
        Act::AllowTest => match allow(model, &finding) {
            Some(run) => run,
            None => {
                let Some(check) = finding.checks.iter().find(|check| check.mark != Mark::Met)
                else {
                    return Vec::new();
                };
                prompt(
                    on,
                    format!("Run the check `{}` for finding {address}.", check.text),
                )
            }
        },
        Act::Dismiss { reason } => prompt(
            on,
            format!("Not a problem: finding {address}: {title}. {reason} Do not raise it again."),
        ),
    };
    let triage = &mut model.findings.triage;
    match act {
        Act::Dismiss { reason } => triage.decide(id, Decision::Dismissed { reason }),
        Act::Accept | Act::Assign { .. } | Act::AskProof | Act::AllowTest => triage.accept(id),
    }
    let mut out = Vec::new();
    let at = model.wire.clock(model.now);
    model.wire.run(&run, &[], at, &mut out);
    out.into_iter().map(Effect::Wire).collect()
}

fn prompt(thread: &ThreadId, text: String) -> Run {
    let target = Address::new(Kind::Thread, thread.as_str()).map(Target::Thing);
    Run {
        text: Some(text),
        ..Run::new(PROMPT).on(target.unwrap_or_default())
    }
}

/// The waiting ask of the finding's thread that runs one of its checks.
fn allow(model: &Model, finding: &Finding) -> Option<Run> {
    let runs = |text: &str| {
        finding
            .checks
            .iter()
            .any(|check| check.mark == Mark::Waiting && text.contains(check.text.as_str()))
    };
    let ask = model.wire.waiting().find(|ask| {
        ask.thread == finding.thread
            && (runs(&ask.title) || ask.description.as_deref().is_some_and(runs))
    })?;
    let target = Address::new(Kind::Approval, ask.id.as_str()).ok()?;
    Some(Run::new(ALLOW).on(Target::Thing(target)))
}

#[derive(Debug, thiserror::Error)]
pub enum TriageError {
    #[error("could not write the triage file {path}: {kind}")]
    Io { path: PathBuf, kind: io::ErrorKind },
    #[error("could not encode the triage file")]
    Encode,
    #[error("the triage file would be {len} bytes, over {max}")]
    TooBig { len: u64, max: u64 },
}

#[derive(Serialize, Deserialize)]
struct DecisionRow {
    finding: FindingId,
    #[serde(flatten)]
    decision: Decision,
}

#[derive(Serialize, Deserialize)]
struct MadeRow {
    id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    thread: Option<String>,
    evidence: String,
    title: String,
    at: u64,
}

#[derive(Serialize)]
struct FileOut {
    decisions: Vec<DecisionRow>,
    made: Vec<MadeRow>,
    next: u64,
}

/// Writes `triage` whole: a temporary file beside `path`, synced, then renamed over it.
pub fn save(path: &Path, triage: &Triage) -> Result<(), TriageError> {
    let out = FileOut {
        decisions: triage
            .decisions
            .iter()
            .map(|(id, decision)| DecisionRow {
                finding: id.clone(),
                decision: decision.clone(),
            })
            .collect(),
        made: triage
            .made
            .iter()
            .map(|made| MadeRow {
                id: made.id.as_str().to_owned(),
                thread: made
                    .thread
                    .as_ref()
                    .map(|thread| thread.as_str().to_owned()),
                evidence: made.evidence.to_string(),
                title: made.title.clone(),
                at: made.at.0,
            })
            .collect(),
        next: triage.next,
    };
    let bytes = serde_json::to_vec_pretty(&out).map_err(|_| TriageError::Encode)?;
    let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
    if len > MAX_FILE {
        return Err(TriageError::TooBig { len, max: MAX_FILE });
    }
    let io = |error: io::Error| TriageError::Io {
        path: path.to_owned(),
        kind: error.kind(),
    };
    replace(path, &path.with_extension("json.tmp"), &bytes).map_err(io)
}

/// Puts `bytes` at `path` whole or not at all, through `temporary`: created new, synced,
/// renamed over `path`, the folder synced; removed on any failure.
pub fn replace(path: &Path, temporary: &Path, bytes: &[u8]) -> io::Result<()> {
    // Invariant: what is at the temporary name is removed, never written through.
    let _ = fs::remove_file(temporary);
    let written = File::options()
        .write(true)
        .create_new(true)
        .open(temporary)
        .and_then(|mut file| file.write_all(bytes).and_then(|()| file.sync_all()))
        .and_then(|()| fs::rename(temporary, path));
    if written.is_err() {
        let _ = fs::remove_file(temporary);
    }
    written?;
    match path
        .parent()
        .filter(|folder| !folder.as_os_str().is_empty())
    {
        Some(folder) => File::open(folder)?.sync_all(),
        None => Ok(()),
    }
}

/// What reading the triage file came to.
#[derive(Debug, Clone, PartialEq)]
pub struct Loaded {
    pub triage: Triage,
    /// False when a file is there that did not read and could not be moved aside: a save
    /// would replace the one copy of what the user decided.
    pub may_save: bool,
}

/// Reads the triage file back, as [`open`] does, for a caller that never saves.
pub fn load(path: &Path) -> Triage {
    open(path).triage
}

/// Reads the triage file back. A missing file is empty triage; an oversized or garbled one
/// is too, and is moved to `<name>.bad` first so no save replaces it. A bad entry is skipped.
pub fn open(path: &Path) -> Loaded {
    let empty = |may_save| Loaded {
        triage: Triage::default(),
        may_save,
    };
    let mut text = String::new();
    let read = File::open(path).and_then(|file| {
        file.take(MAX_FILE.saturating_add(1))
            .read_to_string(&mut text)
    });
    let file = match read {
        Err(error) if error.kind() == io::ErrorKind::NotFound => return empty(true),
        Ok(len) if u64::try_from(len).is_ok_and(|len| len <= MAX_FILE) => {
            serde_json::from_str::<Value>(&text).ok()
        }
        Ok(_) | Err(_) => None,
    };
    let Some(file) = file else {
        // Incident: a file that did not read loaded as empty, and the next save renamed over it.
        let aside = fs::rename(path, path.with_extension("json.bad"));
        return empty(aside.is_ok());
    };
    let rows = |name: &str| {
        let rows = file
            .get(name)
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default();
        let skip = rows.len().saturating_sub(MAX_ENTRIES);
        rows.into_iter().skip(skip)
    };
    let decisions = rows("decisions")
        .filter_map(|row| serde_json::from_value::<DecisionRow>(row).ok())
        .map(|row| (row.finding, row.decision))
        .collect();
    let made: Vec<Made> = rows("made")
        .filter_map(|row| serde_json::from_value::<MadeRow>(row).ok())
        .filter_map(|row| {
            Some(Made {
                id: FindingId::new(row.id),
                thread: row.thread.map(ThreadId::new),
                evidence: row.evidence.parse().ok()?,
                title: row.title,
                at: Millis(row.at),
            })
        })
        .collect();
    // Invariant: a made id is never handed out twice, whatever the file's own counter says.
    let past = made
        .iter()
        .filter_map(|made| made.id.as_str().strip_prefix("made/")?.parse::<u64>().ok())
        .map(|n| n.saturating_add(1))
        .max()
        .unwrap_or(0);
    let next = file
        .get("next")
        .and_then(Value::as_u64)
        .unwrap_or(0)
        .max(past);
    Loaded {
        triage: Triage {
            decisions,
            made,
            next,
            revision: 0,
        },
        may_save: true,
    }
}

#[cfg(test)]
#[path = "triage/tests.rs"]
mod tests;
