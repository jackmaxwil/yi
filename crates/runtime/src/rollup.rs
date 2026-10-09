//! Cross-session spend: every assistant reply under a sessions directory, read off the JSONL
//! ledgers themselves. No index and no collector; the files are the ledger.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, Usage};
use yi_types::wire::{JsonlV4Header, Mutation};

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

#[derive(Default)]
pub struct Scan {
    pub requests: Vec<Request>,
    pub files: usize,
    /// Files whose header would not read: none of their replies are in `requests`.
    pub unreadable: usize,
    /// Lines naming an assistant that did not parse as a session entry.
    pub bad_lines: usize,
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

/// Every reply dated `since_ms` or later under `dir`, children and legacy `rlm-*/sub-*` files
/// included, each counted once by its response id.
pub fn scan(dir: &Path, since_ms: u64) -> Scan {
    let mut files = Vec::new();
    jsonl_files(dir, since_ms, &mut files);
    files.sort();
    let mut scan = Scan::default();
    let mut seen = HashSet::new();
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
        for line in lines.filter(|line| line.contains("\"assistant\"")) {
            let parsed = match serde_json::from_str::<Mutation>(line) {
                Ok(parsed) => parsed,
                Err(_) => {
                    scan.bad_lines = scan.bad_lines.saturating_add(1);
                    continue;
                }
            };
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
            let upstream = diagnostics
                .iter()
                .flatten()
                .find(|note| note.diagnostic_type == "upstream")
                .and_then(|note| note.details.as_ref()?.get("provider")?.as_str())
                .map(str::to_owned);
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

/// Local midnight at or before `now_ms`, as UTC milliseconds.
pub fn local_midnight_ms(now_ms: u64) -> u64 {
    now_ms.saturating_sub(Zone::local().wall_ms(now_ms) % DAY_MS)
}

/// `YYYY-MM-DD` of `at_ms` on the local calendar.
pub fn day_label(zone: &Zone, at_ms: u64) -> String {
    let (year, month, day) = yi_kernel::client::civil_from_days(zone.wall_ms(at_ms) / DAY_MS);
    format!("{year:04}-{month:02}-{day:02}")
}

/// Billed dollars since local midnight across `dir`, and whether any of those replies came back
/// without usage, which makes the sum a lower bound.
pub fn spent_today(dir: &Path, now_ms: u64) -> (f64, bool) {
    let scan = scan(dir, local_midnight_ms(now_ms));
    let unknown = scan.requests.iter().any(|request| request.usage.unknown);
    (scan.requests.iter().map(Request::billed).sum(), unknown)
}
