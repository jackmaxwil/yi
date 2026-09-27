use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_runtime::Wall;
use yi_runtime::fetch::Resolver;
use yi_runtime::plan::authority::{Submission, Unhosted, cli_args, submit};
use yi_runtime::plan::ledger::{self, Report};
use yi_runtime::plan::ops::{Actor, PlanEngine, dispatch_width};
use yi_runtime::plan::snapshot::shadow_tree;
use yi_runtime::plan::store::PlanStore;
use yi_types::plan::doc::{Plan, PlanId, PlanState};
use yi_types::plan::ledger::PlanOpRecord;

pub struct Options {
    pub cwd: PathBuf,
    pub plans_dir: Option<PathBuf>,
    pub session_dir: PathBuf,
    pub json: bool,
}

struct Reply {
    plan: String,
    revision: u64,
    text: String,
    notices: Vec<String>,
}

pub fn run(subcommand: &str, options: &Options) -> i32 {
    let (verb, id) = split(subcommand);
    match verb {
        "lint" => with_plan(options, id, |plan| lint(plan, options)),
        "report" => with_plan(options, id, |plan| report(plan, options)),
        "" => {
            eprintln!(
                "usage: yi plan lint|report [<plan>] | yi plan fuse reset [<plan>] | yi plan repair [<plan>] [<json>] | yi plan accept [<plan>] <json> | yi plan resolve [<plan>] <json> | yi plan <op> [<plan>] [<json args>]"
            );
            2
        }
        _ => apply(subcommand, options),
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

/// Invariant: this process holds no human's prompt, so every op applies here as the owner and
/// an administrative one is refused; the daemon socket cannot carry the answer, so none is dialed.
fn apply(line: &str, options: &Options) -> i32 {
    let reply = cli_args(line).and_then(|mut args| {
        import_source(&mut args, options);
        let plans = dir(options);
        let store = PlanStore::open(plans.clone()).map_err(|error| error.to_string())?;
        let resolver =
            Resolver::new(options.cwd.clone(), Wall::default()).with_plans_dir(plans.clone());
        let mut engine = PlanEngine::new(store, Arc::new(Unhosted))
            .unhosted()
            .with_cwd(options.cwd.clone())
            .with_output_resolve(Arc::new(resolver));
        // The same snapshot the session mints (plan section 6.5): a contracted `done` from the
        // CLI reads the shadow gitdir tree, not a walk of the whole workspace.
        if let Some(snapshotter) = std::env::var_os("HOME")
            .and_then(|home| shadow_tree(&PathBuf::from(home), &options.cwd, &plans))
        {
            engine = engine.with_snapshotter(snapshotter);
        }
        let submission = Submission {
            args,
            request_id: None,
            expected_revision: None,
        };
        match submit(&engine, &Actor::Owner, None, submission) {
            Ok(applied) => Ok(Reply {
                plan: applied.outcome.plan.id.as_str().to_owned(),
                revision: applied.outcome.plan.touched.0,
                text: applied.text(),
                notices: applied.outcome.notices,
            }),
            Err(error) => Err(error.to_string()),
        }
    });
    let reply = match reply {
        Ok(reply) => reply,
        Err(error) => {
            eprintln!("error: {error}");
            return 1;
        }
    };
    if options.json {
        println!(
            "{}",
            json!({
                "plan": reply.plan,
                "revision": reply.revision,
                "text": reply.text,
                "notices": reply.notices,
            })
        );
        return 0;
    }
    for notice in &reply.notices {
        println!("{notice}");
    }
    println!("{}", reply.text);
    0
}

/// `yi plan import <id>` reads `<plans dir>/<id>.md`, the file a format-1 read names;
/// `yi plan import local://<path>` names the file itself, as the missing-journal notice prints.
fn import_source(args: &mut Map<String, Value>, options: &Options) {
    if args.get("op") != Some(&Value::String("import".to_owned())) || args.contains_key("source") {
        return;
    }
    let Some(Value::String(word)) = args.remove("plan") else {
        return;
    };
    let source = if word.contains("://") {
        word
    } else {
        format!(
            "local://{}",
            dir(options).join(format!("{word}.md")).display()
        )
    };
    args.insert("source".to_owned(), Value::String(source));
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
    // Invariant: a root this pass could not read is named rather than counted as absent, so
    // a refused plan says why instead of reading as an empty directory.
    let mut refused = None;
    let chosen = match id {
        Some(name) => PlanId::new(name).ok(),
        None => store
            .roots()
            .unwrap_or_default()
            .into_iter()
            .find(|id| match store.read(id) {
                Ok(plan) => plan.state == PlanState::Active,
                Err(error) => {
                    refused.get_or_insert(error);
                    false
                }
            }),
    };
    let Some(chosen) = chosen else {
        match refused {
            Some(error) => eprintln!("error: {error}"),
            None => eprintln!("error: no plan is open under {}", dir(options).display()),
        }
        return 1;
    };
    match store.read(&chosen) {
        Ok(plan) => run(&plan),
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn lint(plan: &Plan, options: &Options) -> i32 {
    let findings = ledger::lint(plan, dispatch_width().get());
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
    // Advisory, never a verdict (D50): a failing lint would adjudicate beside the step table.
    0
}

fn report(plan: &Plan, options: &Options) -> i32 {
    let measured = ledger::report(plan, &plan_records(options, &plan.id));
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
            "  {:<40} run {:>7} ms  wait {:>7} ms  blocked {:>7} ms  retries {}{}",
            todo.label.as_str(),
            todo.running_ms,
            todo.waiting_ms,
            todo.blocked_ms,
            todo.retries,
            todo.resolution
                .map(|resolution| format!("  {resolution}"))
                .unwrap_or_default()
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
            "resolution": todo.resolution.map(|resolution| resolution.to_string()),
        })).collect::<Vec<_>>(),
    })
}

/// Incident: reading the workspace's newest session measured the one the report was asked
/// from, not the one the plan ran under. The owner wrote a record for this plan, newest first.
fn plan_records(options: &Options, plan: &PlanId) -> Vec<PlanOpRecord> {
    let mut repo = yi_runtime::session_store::JsonlRepo::new(
        options.session_dir.clone(),
        options.cwd.to_string_lossy().into_owned(),
    );
    let Ok(mut listed) = yi_runtime::session_store::SessionRepo::list(&mut repo) else {
        return Vec::new();
    };
    listed.sort_by_key(|metadata| std::cmp::Reverse(metadata.created_at));
    for metadata in &listed {
        let Ok(session) = yi_runtime::session_store::SessionRepo::open(&mut repo, &metadata.id)
        else {
            continue;
        };
        let records = ledger::records(&session);
        if records.iter().any(|record| record.plan == *plan) {
            return records;
        }
    }
    Vec::new()
}

#[cfg(test)]
#[path = "../../types/tests/support/scratch.rs"]
mod scratch;

#[cfg(test)]
mod tests {
    use super::scratch::Scratch;
    use super::*;
    use yi_runtime::session_store::{CreateOptions, JsonlRepo, SessionRepo, lock_session};
    use yi_types::plan::doc::TodoLabel;
    use yi_types::plan::ledger::PLAN_OP_ENTRY_TYPE;

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn op(plan: &PlanId) -> PlanOpRecord {
        PlanOpRecord {
            plan: plan.clone(),
            op: "init".to_owned(),
            actor: "host".to_owned(),
            at: 1,
            todo: TodoLabel::new("Cut the seam").ok(),
            from: None,
            to: None,
            todos: 1,
            extra: serde_json::Map::new(),
        }
    }

    #[test]
    fn a_report_reads_the_session_that_ran_the_plan() -> TestResult {
        let root = Scratch::new("yi-plan-report")?;
        let sessions = root.join("sessions");
        let cwd = root.join("project");
        std::fs::create_dir_all(&sessions)?;
        std::fs::create_dir_all(&cwd)?;
        let plan = PlanId::new("ship-the-widget")?;
        let mut repo = JsonlRepo::new(sessions.clone(), cwd.to_string_lossy().into_owned());
        let owner = repo.create(CreateOptions {
            id: Some("owner".to_owned()),
            parent_session_id: None,
            metadata: None,
        })?;
        lock_session(&owner).append_custom(
            "main",
            PLAN_OP_ENTRY_TYPE,
            Some(serde_json::to_value(op(&plan))?),
        )?;
        // The newest session is the one this report is being asked from, and it
        // is a different millisecond, not a different ordering rule.
        std::thread::sleep(std::time::Duration::from_millis(2));
        repo.create(CreateOptions {
            id: Some("asking".to_owned()),
            parent_session_id: None,
            metadata: None,
        })?;
        let options = Options {
            cwd,
            plans_dir: None,
            session_dir: sessions,
            json: true,
        };
        let records = plan_records(&options, &plan);
        assert_eq!(
            records.len(),
            1,
            "the plan's own session carries its op stream, whatever ran last"
        );
        Ok(())
    }
}
