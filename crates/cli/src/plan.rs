use std::num::NonZeroUsize;
use std::path::PathBuf;

use yi_runtime::plan::ledger::{self, Report};
use yi_runtime::plan::ops::dispatch_width;
use yi_runtime::plan::store::PlanStore;
use yi_types::plan::doc::{Plan, PlanState};

pub struct Options {
    pub cwd: PathBuf,
    pub plans_dir: Option<PathBuf>,
    pub session_dir: PathBuf,
    pub json: bool,
}

pub fn run(subcommand: &str, options: &Options) -> i32 {
    let (verb, id) = split(subcommand);
    match verb {
        "lint" => with_plan(options, id, |plan| lint(plan, options)),
        "report" => with_plan(options, id, |plan| report(plan, options)),
        _ => {
            eprintln!("usage: yi plan lint [<plan>] | yi plan report [<plan>]");
            2
        }
    }
}

fn split(subcommand: &str) -> (&str, Option<&str>) {
    let mut parts = subcommand.split_whitespace();
    (parts.next().unwrap_or_default(), parts.next())
}

fn dir(options: &Options) -> PathBuf {
    options
        .plans_dir
        .clone()
        .unwrap_or_else(|| options.cwd.join(yi_runtime::plan::PLANS_DIR))
}

/// Named, or the one Active root — the same resolution the tool's own unnamed
/// ops use, so `yi plan lint` and the model see the same plan.
fn with_plan(options: &Options, id: Option<&str>, run: impl FnOnce(&Plan) -> i32) -> i32 {
    let store = match PlanStore::open(dir(options)) {
        Ok(store) => store,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    let chosen = match id {
        Some(name) => yi_types::plan::doc::PlanId::new(name).ok(),
        None => store.roots().ok().and_then(|roots| {
            roots.into_iter().find(|id| {
                store
                    .read(id)
                    .is_ok_and(|file| file.plan.state == PlanState::Active)
            })
        }),
    };
    let Some(chosen) = chosen else {
        eprintln!("error: no plan is open under {}", dir(options).display());
        return 1;
    };
    match store.read(&chosen) {
        Ok(file) => run(&file.plan),
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn lint(plan: &Plan, options: &Options) -> i32 {
    let cores = std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN);
    let findings = ledger::lint(plan, dispatch_width(cores).get());
    if options.json {
        let rows: Vec<serde_json::Value> = findings
            .iter()
            .map(|finding| {
                serde_json::json!({
                    "todo": finding.todo.as_ref().map(|label| label.as_str()),
                    "rule": finding.rule,
                    "detail": finding.detail,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::json!({ "plan": plan.id.as_str(), "findings": rows })
        );
        return 0;
    }
    if findings.is_empty() {
        println!("{}: nothing mechanical to report", plan.id);
        return 0;
    }
    for finding in &findings {
        match &finding.todo {
            Some(label) => println!("{}: {} — {}", label, finding.rule, finding.detail),
            None => println!("{}: {}", finding.rule, finding.detail),
        }
    }
    // Advisory, never a verdict (D50): a lint that could fail a build would be
    // a second adjudicator beside the step table.
    0
}

fn report(plan: &Plan, options: &Options) -> i32 {
    let records = match latest_session(options) {
        Some(session) => ledger::records(&session),
        None => Vec::new(),
    };
    let measured = ledger::report(plan, &records);
    if options.json {
        println!("{}", as_json(plan, &measured));
        return 0;
    }
    if measured.todos.is_empty() {
        println!("{}: no op events recorded for this plan", plan.id);
        return 0;
    }
    println!(
        "{}: {} ms wall, {} ms critical path{}",
        plan.id,
        measured.wall_ms,
        measured.critical_path_ms,
        measured
            .serial_fraction()
            .map(|fraction| format!(" (serial fraction {fraction:.2})"))
            .unwrap_or_default()
    );
    if let Some(ratio) = measured.discovery_ratio() {
        println!(
            "  discovery {ratio:.2} ({} at init, {} at the widest)",
            measured.at_init, measured.widest
        );
    }
    for todo in &measured.todos {
        println!(
            "  {:<40} run {:>7} ms  wait {:>7} ms  blocked {:>7} ms  retries {}",
            todo.label.as_str(),
            todo.running_ms,
            todo.waiting_ms,
            todo.blocked_ms,
            todo.retries
        );
    }
    0
}

fn as_json(plan: &Plan, measured: &Report) -> serde_json::Value {
    serde_json::json!({
        "plan": plan.id.as_str(),
        "wallMs": measured.wall_ms,
        "criticalPathMs": measured.critical_path_ms,
        "serialFraction": measured.serial_fraction(),
        "discoveryRatio": measured.discovery_ratio(),
        "todosAtInit": measured.at_init,
        "todosAtWidest": measured.widest,
        "todos": measured.todos.iter().map(|todo| serde_json::json!({
            "label": todo.label.as_str(),
            "runningMs": todo.running_ms,
            "waitingMs": todo.waiting_ms,
            "blockedMs": todo.blocked_ms,
            "retries": todo.retries,
            "ended": todo.ended.as_ref().map(yi_types::plan::doc::TodoStateName::as_str),
        })).collect::<Vec<_>>(),
    })
}

/// The op stream lives on the session that wrote it, so a report with no
/// session named reads the most recent one for this workspace.
fn latest_session(options: &Options) -> Option<yi_runtime::session_store::SharedSession> {
    let mut repo = yi_runtime::session_store::JsonlRepo::new(
        options.session_dir.clone(),
        options.cwd.to_string_lossy().into_owned(),
    );
    let listed = yi_runtime::session_store::SessionRepo::list(&mut repo).ok()?;
    let newest = listed
        .iter()
        .max_by_key(|metadata| metadata.created_at)?
        .id
        .clone();
    yi_runtime::session_store::SessionRepo::open(&mut repo, &newest).ok()
}
