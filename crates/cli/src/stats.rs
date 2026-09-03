//! `yi stats [session-id]` — replay one session's JSONL for per-tool latency, failure kinds,
//! truncation, shell categories, edit mix and tokens. No collector: the file is the ledger.

use std::collections::BTreeMap;

use serde_json::{Value, json};
use yi_runtime::session_store::{
    BranchBounds, EntryOrder, EntryQuery, JsonlRepo, SessionRepo, lock_session,
};
use yi_types::entry::Entry;
use yi_types::message::AgentMessage;

#[derive(Default)]
struct ToolRow {
    calls: u64,
    errors: u64,
    error_kinds: BTreeMap<String, u64>,
    durations_ms: Vec<u64>,
    out_bytes: u64,
    truncated: u64,
}

#[derive(Default)]
struct Totals {
    turns: u64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    cost: f64,
    categories: BTreeMap<String, u64>,
    edit_ops: BTreeMap<String, u64>,
}

fn percentile(sorted: &[u64], fraction: f64) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = ((sorted.len() - 1) as f64 * fraction).round() as usize;
    sorted.get(rank.min(sorted.len() - 1)).copied().unwrap_or(0)
}

fn truncated_in(details: &Value) -> bool {
    ["truncated", "byteCapped", "collectionCapped", "capped"]
        .iter()
        .any(|key| details.get(key).and_then(Value::as_bool).unwrap_or(false))
}

fn reduce_entry(entry: &Entry, tools: &mut BTreeMap<String, ToolRow>, totals: &mut Totals) {
    let Entry::Message { message, .. } = entry else {
        return;
    };
    match message {
        AgentMessage::Assistant { usage, .. } => {
            totals.turns = totals.turns.saturating_add(1);
            totals.input = totals.input.saturating_add(usage.input);
            totals.output = totals.output.saturating_add(usage.output);
            totals.cache_read = totals.cache_read.saturating_add(usage.cache_read);
            totals.cache_write = totals.cache_write.saturating_add(usage.cache_write);
            totals.cost += usage.cost.total.as_f64().unwrap_or(0.0);
        }
        AgentMessage::ToolResult {
            tool_name,
            details,
            is_error,
            ..
        } => {
            let row = tools.entry(tool_name.clone()).or_default();
            row.calls = row.calls.saturating_add(1);
            let details = details.clone().unwrap_or(Value::Null);
            if *is_error {
                row.errors = row.errors.saturating_add(1);
                let kind = details
                    .get("errorKind")
                    .and_then(Value::as_str)
                    .unwrap_or("tool_error");
                let seen = row.error_kinds.entry(kind.to_owned()).or_default();
                *seen = seen.saturating_add(1);
            }
            if let Some(duration) = details.get("durationMs").and_then(Value::as_u64) {
                row.durations_ms.push(duration);
            }
            row.out_bytes = row
                .out_bytes
                .saturating_add(details.get("outBytes").and_then(Value::as_u64).unwrap_or(0));
            if truncated_in(&details) {
                row.truncated = row.truncated.saturating_add(1);
            }
            if let Some(category) = details.get("category").and_then(Value::as_str) {
                let seen = totals.categories.entry(category.to_owned()).or_default();
                *seen = seen.saturating_add(1);
            }
            if let Some(ops) = details.get("ops").and_then(Value::as_object) {
                for (op, count) in ops {
                    let seen = totals.edit_ops.entry(op.clone()).or_default();
                    *seen = seen.saturating_add(count.as_u64().unwrap_or(0));
                }
            }
        }
        _ => {}
    }
}

pub struct Options {
    pub session_dir: std::path::PathBuf,
    pub cwd: String,
    pub json: bool,
}

pub fn run(id_arg: &str, options: &Options) -> i32 {
    let mut repo = JsonlRepo::new(options.session_dir.clone(), options.cwd.clone());
    let id = if id_arg.trim().is_empty() {
        match crate::sessions::latest_id(&mut repo) {
            Some(id) => id,
            None => {
                eprintln!("error: no sessions for this directory");
                return 1;
            }
        }
    } else {
        id_arg.trim().to_owned()
    };
    let store = match repo.open(&id) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let session = lock_session(&store);
    let entries = match session.find_entries_on_branch(
        "main",
        &EntryQuery {
            order: EntryOrder::OldestFirst,
            ..EntryQuery::default()
        },
        &BranchBounds::default(),
    ) {
        Ok(entries) => entries,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let mut tools: BTreeMap<String, ToolRow> = BTreeMap::new();
    let mut totals = Totals::default();
    for entry in &entries {
        reduce_entry(entry, &mut tools, &mut totals);
    }
    for row in tools.values_mut() {
        row.durations_ms.sort_unstable();
    }
    if options.json {
        let tool_rows: Vec<Value> = tools
            .iter()
            .map(|(name, row)| {
                json!({
                    "tool": name,
                    "calls": row.calls,
                    "errors": row.errors,
                    "errorKinds": row.error_kinds,
                    "p50Ms": percentile(&row.durations_ms, 0.5),
                    "p95Ms": percentile(&row.durations_ms, 0.95),
                    "outBytes": row.out_bytes,
                    "truncated": row.truncated,
                })
            })
            .collect();
        let value = json!({
            "session": id,
            "tools": tool_rows,
            "turns": totals.turns,
            "tokens": {
                "input": totals.input,
                "output": totals.output,
                "cacheRead": totals.cache_read,
                "cacheWrite": totals.cache_write,
            },
            "cost": totals.cost,
            "bashCategories": totals.categories,
            "editOps": totals.edit_ops,
        });
        if let Ok(line) = serde_json::to_string(&value) {
            println!("{line}");
        }
        return 0;
    }
    println!("session {id}");
    println!(
        "turns {}  tokens in {} out {} cache-read {} cache-write {}  cost ${:.4}",
        totals.turns,
        totals.input,
        totals.output,
        totals.cache_read,
        totals.cache_write,
        totals.cost
    );
    if tools.is_empty() {
        println!("no tool calls");
        return 0;
    }
    println!(
        "{:<10} {:>6} {:>6} {:>8} {:>8} {:>10} {:>6}  kinds",
        "tool", "calls", "errors", "p50ms", "p95ms", "outBytes", "trunc"
    );
    let mut rows: Vec<(&String, &ToolRow)> = tools.iter().collect();
    rows.sort_by(|a, b| b.1.calls.cmp(&a.1.calls));
    for (name, row) in rows {
        let kinds: Vec<String> = row
            .error_kinds
            .iter()
            .map(|(kind, count)| format!("{kind}:{count}"))
            .collect();
        println!(
            "{name:<10} {:>6} {:>6} {:>8} {:>8} {:>10} {:>6}  {}",
            row.calls,
            row.errors,
            percentile(&row.durations_ms, 0.5),
            percentile(&row.durations_ms, 0.95),
            row.out_bytes,
            row.truncated,
            kinds.join(" ")
        );
    }
    if !totals.categories.is_empty() {
        let parts: Vec<String> = totals
            .categories
            .iter()
            .map(|(category, count)| format!("{category}:{count}"))
            .collect();
        println!("bash categories  {}", parts.join(" "));
    }
    if !totals.edit_ops.is_empty() {
        let parts: Vec<String> = totals
            .edit_ops
            .iter()
            .map(|(op, count)| format!("{op}:{count}"))
            .collect();
        println!("edit ops  {}", parts.join(" "));
    }
    0
}
