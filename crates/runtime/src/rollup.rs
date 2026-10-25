//! Cross-session spend: every assistant reply under a sessions directory, read off the JSONL
//! ledgers themselves. No index and no collector; the files are the ledger.

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Usage};
use yi_types::plan::canonical::sorted;
use yi_types::plan::doc::OWNER_AGENT;
use yi_types::plan::ledger::{JournalRecord, PLAN_OP_ENTRY_TYPE, PlanOpRecord};
use yi_types::wire::{JsonlV4Header, Mutation};

use crate::cache_miss::diagnostic_detail;
use crate::plan::journal::{Journal, RealFs};
use crate::plan::ops::{CLASSIFIER_ACTOR, ENGINE_AGENT, HOST_ACTOR};
use crate::plan::state::{KIND_DONE_REFUSED, KIND_VERIFICATION_STALE};
use crate::plan::store::JOURNAL_NAME;
use crate::schedule::Zone;

pub use yi_session::now_ms;

pub const DAY_MS: u64 = 86_400_000;

/// One billed assistant reply. Parent `usage` records (`child_usage_attributed` among them) are
/// never read: a child's replies are counted once, from the child's own file.
pub struct Request {
    pub session: String,
    pub child: bool,
    pub at_ms: u64,
    pub provider: String,
    pub model: String,
    /// The `upstream` diagnostic's `provider`, which OpenRouter reports per reply.
    pub upstream: Option<String>,
    pub usage: Usage,
}

impl Request {
    pub fn billed(&self) -> f64 {
        self.usage.cost.total.as_f64().unwrap_or(0.0)
    }

    pub fn prompt(&self) -> i64 {
        self.usage
            .input
            .saturating_add(self.usage.cache_read)
            .saturating_add(self.usage.cache_write)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ActorClass {
    Owner,
    Engine,
    Child,
    Other,
}

impl ActorClass {
    /// Invariant: the engine stamps a `user://` citation, `classifier` or `host` for the
    /// approvers; every other word that is not the owner or the engine names a child.
    fn of(actor: &str) -> Self {
        match actor {
            OWNER_AGENT => Self::Owner,
            ENGINE_AGENT => Self::Engine,
            CLASSIFIER_ACTOR | HOST_ACTOR => Self::Other,
            word if word.starts_with("user://") => Self::Other,
            _ => Self::Child,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Engine => "engine",
            Self::Child => "child",
            Self::Other => "other",
        }
    }
}

#[derive(Default)]
pub struct Scan {
    pub requests: Vec<Request>,
    pub files: usize,
    /// Files whose header would not read: none of their replies are in `requests`.
    pub unreadable: usize,
    /// Lines naming an assistant or a plan op that did not parse.
    pub bad_lines: usize,
    /// Applied plan ops by (op, who applied it); an op the table rejected reads `<op> (rejected)`.
    pub plan_ops: BTreeMap<(String, ActorClass), u64>,
    pub journals: usize,
    plan_seen: HashSet<String>,
}

impl Scan {
    /// Invariant: a journal record, its session mirror and a fork's copy are one op and count once,
    /// though the journal sorts a record's keys and the mirror keeps the order it was stamped in.
    fn add_plan_op(&mut self, record: &PlanOpRecord, since_ms: u64) {
        if record.at < since_ms
            || !serde_json::to_value(record)
                .is_ok_and(|value| self.plan_seen.insert(sorted(&value).to_string()))
        {
            return;
        }
        let op = match (record.is_refusal(), record.op.as_str()) {
            (true, KIND_DONE_REFUSED | KIND_VERIFICATION_STALE) | (false, _) => record.op.clone(),
            (true, rejected) => format!("{rejected} (rejected)"),
        };
        let count = self
            .plan_ops
            .entry((op, ActorClass::of(&record.actor)))
            .or_insert(0);
        *count = count.saturating_add(1);
    }
}

fn jsonl_files(dir: &Path, since_ms: u64, out: &mut Vec<PathBuf>) {
    let mut pending = vec![dir.to_path_buf()];
    while let Some(dir) = pending.pop() {
        let Ok(read) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in read.flatten() {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() {
                pending.push(path);
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".jsonl") || name.ends_with(".telemetry.jsonl") {
                continue;
            }
            // Only an append grows a session, so a file untouched since `since_ms` holds nothing newer.
            let modified = entry
                .metadata()
                .and_then(|meta| meta.modified())
                .ok()
                .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(u64::MAX, |elapsed| {
                    u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX)
                });
            if modified >= since_ms {
                out.push(path);
            }
        }
    }
}

/// The entry's own `timestamp`, read off the line's tail without parsing it: an entry ends
/// `"seq":N,"timestamp":N}`. `None` when the tail is anything else, and the line is parsed whole.
fn entry_stamp(line: &str) -> Option<u64> {
    let tail = line.trim_end().strip_suffix('}')?;
    let (_, digits) = tail.rsplit_once("\"timestamp\":")?;
    digits.parse().ok()
}

/// Every reply dated `since_ms` or later under `dir`, children and legacy `rlm-*/sub-*` files
/// included, each counted once by its response id.
pub fn scan(dir: &Path, since_ms: u64) -> Scan {
    let mut files = Vec::new();
    jsonl_files(dir, since_ms, &mut files);
    files.sort();
    let mut scan = Scan::default();
    let mut seen = HashSet::new();
    let plan_op = format!("\"customType\":\"{PLAN_OP_ENTRY_TYPE}\"");
    for path in files {
        let text = std::fs::read(&path)
            .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
            .ok();
        let mut lines = text.as_deref().unwrap_or_default().lines();
        let Some(header) = lines
            .next()
            .and_then(|line| serde_json::from_str::<JsonlV4Header>(line).ok())
        else {
            scan.unreadable = scan.unreadable.saturating_add(1);
            continue;
        };
        scan.files = scan.files.saturating_add(1);
        let child = path.strip_prefix(dir).is_ok_and(|rel| {
            rel.components().any(|part| {
                let part = part.as_os_str().to_string_lossy();
                part == "children" || part.starts_with("sub-")
            })
        });
        for line in lines.filter(|line| line.contains("\"assistant\"") || line.contains(&plan_op)) {
            if entry_stamp(line).is_some_and(|stamp| stamp > 0 && stamp < since_ms) {
                continue;
            }
            let parsed = match serde_json::from_str::<Mutation>(line) {
                Ok(parsed) => parsed,
                Err(_) => {
                    scan.bad_lines = scan.bad_lines.saturating_add(1);
                    continue;
                }
            };
            if let Mutation::Entry {
                entry: Entry::Custom {
                    custom_type, data, ..
                },
                ..
            } = &parsed
            {
                if custom_type == PLAN_OP_ENTRY_TYPE {
                    match data
                        .clone()
                        .and_then(|data| serde_json::from_value::<PlanOpRecord>(data).ok())
                    {
                        Some(record) => scan.add_plan_op(&record, since_ms),
                        None => scan.bad_lines = scan.bad_lines.saturating_add(1),
                    }
                }
                continue;
            }
            let Mutation::Entry {
                entry:
                    Entry::Message {
                        id,
                        message:
                            AgentMessage::Assistant {
                                provider,
                                model,
                                response_id,
                                diagnostics,
                                usage,
                                ..
                            },
                        timestamp,
                        ..
                    },
                ..
            } = parsed
            else {
                continue;
            };
            let at_ms = if timestamp > 0 {
                timestamp
            } else {
                header.created_at
            };
            let key = response_id.unwrap_or_else(|| format!("{}/{id}", header.id));
            if at_ms < since_ms || !seen.insert(key) {
                continue;
            }
            let upstream = diagnostic_detail(diagnostics.as_deref(), "upstream", "provider");
            scan.requests.push(Request {
                session: header.id.clone(),
                child,
                at_ms,
                provider,
                model,
                upstream,
                usage,
            });
        }
    }
    scan
}

/// Adds the plan journals under `plans` (`<plans>/<root>/ops.jsonl`) to `scan.plan_ops`: a session
/// file mirrors only the ops its own engine emitted, so delegations and rejected ops live here.
pub fn fold_journals(scan: &mut Scan, plans: &Path, since_ms: u64) {
    let mut files = Vec::new();
    jsonl_files(plans, since_ms, &mut files);
    files.retain(|path| path.file_name().is_some_and(|name| name == JOURNAL_NAME));
    for path in files {
        let Ok(read) = Journal::open(path, Arc::new(RealFs)).lines() else {
            scan.unreadable = scan.unreadable.saturating_add(1);
            continue;
        };
        scan.journals = scan.journals.saturating_add(1);
        let torn = usize::from(read.torn.is_some());
        scan.bad_lines = scan.bad_lines.saturating_add(torn);
        for bytes in read
            .lines
            .into_iter()
            .map(|line| line.bytes.unwrap_or_default())
        {
            match serde_json::from_slice::<JournalRecord>(&bytes) {
                Ok(journaled) => scan.add_plan_op(&journaled.record, since_ms),
                Err(_) => scan.bad_lines = scan.bad_lines.saturating_add(1),
            }
        }
    }
}

/// Local midnight at or before `now_ms`, as UTC milliseconds.
pub fn local_midnight_ms(now_ms: u64) -> u64 {
    now_ms.saturating_sub(Zone::local().wall_ms(now_ms) % DAY_MS)
}

/// `YYYY-MM-DD` of `at_ms` on the local calendar.
pub fn day_label(zone: &Zone, at_ms: u64) -> String {
    let (year, month, day) = yi_kernel::client::civil_from_days(zone.wall_ms(at_ms) / DAY_MS);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Billed dollars since local midnight across `dir`, and whether a reply lacked usage (a lower
/// bound). Each call reads the files afresh: a daemon's later sessions must see earlier spend.
pub fn spent_today(dir: &Path, now_ms: u64) -> (f64, bool) {
    let scan = scan(dir, local_midnight_ms(now_ms));
    let unknown = scan.requests.iter().any(|request| request.usage.unknown);
    (scan.requests.iter().map(Request::billed).sum(), unknown)
}
