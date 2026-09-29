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
struct Tokens {
    turns: u64,
    input: i64,
    output: i64,
    cache_read: i64,
    cache_write: i64,
    cost: f64,
    /// A same-model request after the first that read nothing back: the prefix broke or
    /// never cleared the provider's minimum, and either way it was paid for twice.
    misses: u64,
}

impl Tokens {
    fn add(&mut self, usage: &yi_types::message::Usage, miss: bool) {
        self.turns = self.turns.saturating_add(1);
        self.input = self.input.saturating_add(usage.input);
        self.output = self.output.saturating_add(usage.output);
        self.cache_read = self.cache_read.saturating_add(usage.cache_read);
        self.cache_write = self.cache_write.saturating_add(usage.cache_write);
        self.cost += usage.cost.total.as_f64().unwrap_or(0.0);
        self.misses = self.misses.saturating_add(u64::from(miss));
    }

    fn hit_rate(&self) -> f64 {
        share(self.cache_read, self)
    }

    fn write_share(&self) -> f64 {
        share(self.cache_write, self)
    }

    fn json(&self) -> Value {
        json!({
            "turns": self.turns,
            "input": self.input,
            "output": self.output,
            "cacheRead": self.cache_read,
            "cacheWrite": self.cache_write,
            "cost": self.cost,
            "hitRate": self.hit_rate(),
            "writeShare": self.write_share(),
            "misses": self.misses,
        })
    }
}

impl Tokens {
    fn line(&self) -> String {
        format!(
            "turns {}  tokens in {} out {} cache-read {} cache-write {}  cost ${:.4}  cache-hit {:.1}% write {:.1}% misses {}",
            self.turns,
            self.input,
            self.output,
            self.cache_read,
            self.cache_write,
            self.cost,
            self.hit_rate() * 100.0,
            self.write_share() * 100.0,
            self.misses
        )
    }
}

impl Totals {
    fn cost(&self) -> f64 {
        self.all.cost
    }
}

fn share(part: i64, tokens: &Tokens) -> f64 {
    let whole = tokens
        .input
        .saturating_add(tokens.cache_read)
        .saturating_add(tokens.cache_write);
    if whole > 0 {
        part as f64 / whole as f64
    } else {
        0.0
    }
}

#[derive(Default)]
struct Totals {
    all: Tokens,
    by_model: BTreeMap<String, Tokens>,
    last_model: Option<String>,
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
        AgentMessage::Assistant {
            usage,
            provider,
            model,
            ..
        } => {
            let key = format!("{provider}/{model}");
            let miss = usage.cache_read == 0
                && !usage.unknown
                && totals.last_model.as_deref() == Some(key.as_str());
            totals.all.add(usage, miss);
            totals
                .by_model
                .entry(key.clone())
                .or_default()
                .add(usage, miss);
            totals.last_model = Some(key);
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
    if let Some(dir) = id_arg.trim().strip_prefix("telemetry") {
        return telemetry_rollup(std::path::Path::new(dir.trim()), options.json);
    }
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
        let models: Vec<Value> = totals
            .by_model
            .iter()
            .map(|(model, tokens)| {
                let mut row = tokens.json();
                row["model"] = json!(model);
                row
            })
            .collect();
        let value = json!({
            "session": id,
            "tools": tool_rows,
            "turns": totals.all.turns,
            "tokens": {
                "input": totals.all.input,
                "output": totals.all.output,
                "cacheRead": totals.all.cache_read,
                "cacheWrite": totals.all.cache_write,
            },
            "cache": {
                "hitRate": totals.all.hit_rate(),
                "writeShare": totals.all.write_share(),
                "misses": totals.all.misses,
            },
            "models": models,
            "cost": totals.cost(),
            "bashCategories": totals.categories,
            "editOps": totals.edit_ops,
        });
        if let Ok(line) = serde_json::to_string(&value) {
            println!("{line}");
        }
        return 0;
    }
    println!("session {id}");
    println!("{}", totals.all.line());
    if totals.by_model.len() > 1 {
        for (model, tokens) in &totals.by_model {
            println!("  {model}  {}", tokens.line());
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;
    use yi_types::message::{Content, StopReason, Usage};

    fn assistant(cache_read: i64, cache_write: i64) -> Entry {
        let mut usage = Usage::zero();
        usage.input = 1000;
        usage.cache_read = cache_read;
        usage.cache_write = cache_write;
        Entry::Message {
            id: String::new(),
            terminate: None,
            parent_id: None,
            seq: 0,
            timestamp: 0,
            message: AgentMessage::Assistant {
                content: vec![Content::Text {
                    text: "ok".to_owned(),
                    text_signature: None,
                }],
                api: "openai-completions".to_owned(),
                provider: "openrouter".to_owned(),
                model: "z-ai/glm-5.3-flash".to_owned(),
                response_model: None,
                response_id: None,
                diagnostics: None,
                usage,
                stop_reason: StopReason::Stop,
                deferred: None,
                error_message: None,
                raw_stop_reason: None,
                end_turn: None,
                timestamp: 0,
            },
        }
    }

    fn reduce(entries: &[Entry]) -> Totals {
        let mut tools = BTreeMap::new();
        let mut totals = Totals::default();
        for entry in entries {
            reduce_entry(entry, &mut tools, &mut totals);
        }
        totals
    }

    /// The first request is cold by definition; a warm same-model request that
    /// reads nothing is the miss, whatever it wrote.
    #[test]
    fn a_warm_request_that_reads_nothing_is_a_miss() {
        let cold_then_miss = reduce(&[assistant(0, 1000), assistant(0, 0)]);
        assert_eq!(cold_then_miss.all.misses, 1);
        assert_eq!(cold_then_miss.all.hit_rate(), 0.0);
        let cold_then_hit = reduce(&[assistant(0, 1000), assistant(1000, 0)]);
        assert_eq!(cold_then_hit.all.misses, 0);
        assert_eq!(cold_then_hit.all.hit_rate(), 0.25);
        assert_eq!(cold_then_hit.all.write_share(), 0.25);
        assert_eq!(cold_then_hit.by_model.len(), 1);
    }
}

#[derive(Default)]
struct ToolSpans {
    calls: u64,
    errors: u64,
    ms: Vec<u64>,
}

fn telemetry_files(dir: &std::path::Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            telemetry_files(&path, out);
        } else if path.to_string_lossy().ends_with(".telemetry.jsonl") {
            out.push(path);
        }
    }
}

/// `yi stats telemetry <dir>`: every sidecar under `dir` rolled into one record — the numbers
/// a run is judged by (plan 2026-09-05 §3.E), the same formulas as a session's own stats.
fn telemetry_rollup(dir: &std::path::Path, json_out: bool) -> i32 {
    use yi_types::telemetry::{Span, SpanKind};
    let mut files = Vec::new();
    telemetry_files(dir, &mut files);
    files.sort();
    if files.is_empty() {
        eprintln!("error: no *.telemetry.jsonl under {}", dir.display());
        return 1;
    }
    let (mut ttft, mut total, mut requests, mut turns) = (Vec::new(), Vec::new(), 0_u64, 0_u64);
    let mut tokens = Tokens::default();
    let mut cost = 0.0_f64;
    let mut tools: BTreeMap<String, ToolSpans> = BTreeMap::new();
    let mut classes: BTreeMap<String, u64> = BTreeMap::new();
    let mut spans = 0_u64;
    for file in &files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            let Ok(span) = serde_json::from_str::<Span>(line) else {
                continue;
            };
            spans = spans.saturating_add(1);
            if let Some(class) = &span.class {
                *classes.entry(class.clone()).or_default() += 1;
            }
            match span.span {
                SpanKind::Request => {
                    requests = requests.saturating_add(1);
                    ttft.extend(span.ttft_ms);
                    total.extend(span.total_ms);
                    tokens.input = tokens.input.saturating_add(span.input.unwrap_or(0));
                    tokens.output = tokens.output.saturating_add(span.output.unwrap_or(0));
                    tokens.cache_read = tokens
                        .cache_read
                        .saturating_add(span.cache_read.unwrap_or(0));
                    tokens.cache_write = tokens
                        .cache_write
                        .saturating_add(span.cache_write.unwrap_or(0));
                    cost += span.cost_usd.unwrap_or(0.0);
                }
                SpanKind::Tool => {
                    let row = tools
                        .entry(span.tool.clone().unwrap_or_default())
                        .or_default();
                    row.calls = row.calls.saturating_add(1);
                    if span.ok == Some(false) {
                        row.errors = row.errors.saturating_add(1);
                    }
                    row.ms.extend(span.ms);
                }
                SpanKind::Turn => turns = turns.saturating_add(1),
                SpanKind::Compaction | SpanKind::Other(_) => {}
            }
        }
    }
    ttft.sort_unstable();
    total.sort_unstable();
    for row in tools.values_mut() {
        row.ms.sort_unstable();
    }
    let tool_rows: Vec<Value> = tools
        .iter()
        .map(|(name, row)| {
            json!({"tool": name, "calls": row.calls, "errors": row.errors,
                   "p50Ms": percentile(&row.ms, 0.5), "p95Ms": percentile(&row.ms, 0.95)})
        })
        .collect();
    let record = json!({
        "files": files.len(),
        "spans": spans,
        "requests": requests,
        "turns": turns,
        "ttftP50Ms": percentile(&ttft, 0.5),
        "ttftP95Ms": percentile(&ttft, 0.95),
        "totalP50Ms": percentile(&total, 0.5),
        "tokens": tokens.json(),
        "hitRate": tokens.hit_rate(),
        "costUsd": cost,
        "tools": tool_rows,
        "classes": classes,
    });
    if json_out {
        println!("{record}");
        return 0;
    }
    println!(
        "{} file(s), {} span(s): {} request(s), {} turn(s)",
        files.len(),
        spans,
        requests,
        turns
    );
    println!(
        "ttft p50 {} ms · p95 {} ms · total p50 {} ms · cache hit {:.1}% · cost ${cost:.4}",
        percentile(&ttft, 0.5),
        percentile(&ttft, 0.95),
        percentile(&total, 0.5),
        tokens.hit_rate() * 100.0
    );
    for (name, row) in &tools {
        println!(
            "  {name:<12} {:>4} call(s) {:>3} error(s) p50 {} ms",
            row.calls,
            row.errors,
            percentile(&row.ms, 0.5)
        );
    }
    for (class, count) in &classes {
        println!("  class {class}: {count}");
    }
    0
}
