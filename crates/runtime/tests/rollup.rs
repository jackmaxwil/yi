//! The cross-session fold, over a root session and a child recorded by the real session writer
//! (scrubbed of text, in `fixtures/rollup`) laid out as `yi` lays them out on disk.

use std::error::Error;
use std::path::Path;
use std::sync::Arc;

use yi_runtime::rollup::scan;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

const ROOT: &str = include_str!("fixtures/rollup/root_glm_with_child_usage.jsonl");
const CHILD: &str = include_str!("fixtures/rollup/child_bedrock.jsonl");
const CHILD_REPLY: f64 = 0.010216;
/// The root fixture's two replies, summed by hand from its file: 0.0004671 + 0.000159415.
const ROOT_REPLIES: f64 = 0.000626515;

fn put(root: &Path, relative: &str, text: &str) -> std::io::Result<()> {
    let path = root.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, text)
}

/// A root, its child under `children/`, and a legacy `rlm-*/sub-*` child with its own reply id.
fn laid_out(name: &str) -> Result<Scratch, Box<dyn Error>> {
    let dir = Scratch::new(name)?;
    let stem = "--proj--/1787905015970_01a04771-80a2-727b-a35d-9a0bfe11023b";
    put(&dir, &format!("{stem}.jsonl"), ROOT)?;
    put(&dir, &format!("{stem}/children/sub-aa/c.jsonl"), CHILD)?;
    let legacy = CHILD
        .replace("R8jG4wschDCyIUWXZMx1", "legacyReplyId00000000")
        .replace("01a0faa8-5f44", "01a0faa8-aaaa");
    put(&dir, "rlm-2/sub-bb/c.jsonl", &legacy)?;
    Ok(dir)
}

/// Dies when a parent's `child_usage_attributed` records are folded in beside the children's
/// own files (the root fixture holds two worth $0.00022), or a legacy `rlm-*/sub-*` file is missed.
#[test]
fn each_reply_counts_once_across_root_child_and_legacy_files() -> TestResult {
    let dir = laid_out("yi-rollup-layout")?;
    let found = scan(&dir, 0);
    assert_eq!((found.unreadable, found.bad_lines), (0, 0));
    assert_eq!(found.requests.len(), 4);
    let billed: f64 = found.requests.iter().map(|r| r.billed()).sum();
    let want = ROOT_REPLIES + 2.0 * CHILD_REPLY;
    assert!((billed - want).abs() < 1e-9, "{billed} vs {want}");
    assert_eq!(found.requests.iter().filter(|r| r.child).count(), 2);
    let child = found.requests.iter().find(|r| r.child).ok_or("no child")?;
    assert_eq!(child.upstream.as_deref(), Some("Amazon Bedrock"));
    assert_eq!(child.model, "anthropic/claude-opus-5.5");
    Ok(())
}

/// Dies when a copy of a session under a second path doubles its dollars.
#[test]
fn a_reply_copied_into_a_second_file_is_counted_once() -> TestResult {
    let dir = laid_out("yi-rollup-dedupe")?;
    put(&dir, "pr-rounds/x/c.jsonl", CHILD)?;
    assert_eq!(scan(&dir, 0).requests.len(), 4);
    Ok(())
}

/// Dies when `--since` keeps a reply stamped before the window because its file was touched
/// after it: the root file's mtime is now, its replies are from August.
#[test]
fn a_window_keeps_replies_by_their_own_timestamp() -> TestResult {
    let dir = laid_out("yi-rollup-window")?;
    let found = scan(&dir, 1_790_000_000_000);
    assert_eq!(found.requests.len(), 2);
    assert!(found.requests.iter().all(|r| r.child));
    Ok(())
}

/// One bad part among good ones: an empty file with no header and a torn assistant line are
/// named, and the rest still count.
#[test]
fn an_unreadable_file_and_a_torn_line_are_counted_not_swallowed() -> TestResult {
    let dir = laid_out("yi-rollup-bad")?;
    put(&dir, "--proj--/empty.jsonl", "")?;
    let torn =
        format!("{CHILD}{{\"kind\":\"entry\",\"message\":{{\"role\":\"assistant\",\"model\":\n");
    put(
        &dir,
        "rlm-3/sub-cc/c.jsonl",
        &torn.replace("R8jG4wschDCyIUWXZMx1", "tornCopy"),
    )?;
    let found = scan(&dir, 0);
    assert_eq!((found.unreadable, found.bad_lines), (1, 1));
    assert_eq!(found.requests.len(), 5);
    Ok(())
}

/// Dies when an out-of-window line is fully parsed before it is dropped: a line stamped before
/// the window and torn past repair counts as a bad line only if the window did not drop it first.
#[test]
fn a_line_before_the_window_is_dropped_without_being_parsed() -> TestResult {
    let dir = Scratch::new("yi-rollup-cheap")?;
    let torn = "{\"kind\":\"entry\",\"message\":{\"role\":\"assistant\",\"model\":,\"seq\":9,\"timestamp\":5}\n";
    put(
        &dir,
        "a.jsonl",
        &format!(
            "{}{torn}",
            CHILD
                .lines()
                .next()
                .map_or_else(String::new, |h| format!("{h}\n"))
        ),
    )?;
    assert_eq!(scan(&dir, 1_000).bad_lines, 0);
    assert_eq!(scan(&dir, 0).bad_lines, 1);
    Ok(())
}

/// `init` in the plan tool's own wire shape, which is how a delegation is written.
fn open_plan(
    engine: &yi_runtime::plan::ops::PlanEngine,
    todos: serde_json::Value,
) -> Result<(), Box<dyn Error>> {
    let args = serde_json::json!({"op": "init", "goal": "report the run", "todos": todos});
    let submission = yi_runtime::plan::authority::Submission {
        args: args.as_object().cloned().unwrap_or_default(),
        request_id: None,
        expected_revision: None,
    };
    yi_runtime::plan::authority::submit(
        engine,
        &yi_runtime::plan::ops::Actor::Owner,
        None,
        submission,
    )
    .map(|_applied| ())
    .map_err(|error| error.to_string().into())
}

/// Dies when the journal is not folded (`spawn_intent` is never mirrored into a session), when an
/// engine, child or user record reads as the owner's, or when a refusal counts as the op it
/// refused. The journal is written by the real engine; the tally below is the ops applied.
#[test]
fn a_plan_journal_folds_by_kind_and_actor_and_a_refusal_is_not_its_op() -> TestResult {
    use yi_runtime::plan::ops::{Actor, Op, OpRequest, PlanEngine};
    use yi_runtime::plan::store::PlanStore;
    use yi_runtime::rollup::{ActorClass, Scan, fold_journals};
    use yi_types::plan::doc::{AgentId, AttemptId, BlockedOn, TodoLabel};
    let dir = Scratch::new("yi-rollup-journal")?;
    let engine = PlanEngine::new(
        PlanStore::open(dir.to_path_buf())?,
        Arc::new(crate::plan_ops::Stub::default()),
    );
    let apply = |actor: Actor, op: Op| {
        engine.apply(OpRequest {
            plan: None,
            actor,
            op,
            request_id: None,
            expected_revision: None,
        })
    };
    let job = TodoLabel::new("write the report")?;
    let delegated = serde_json::json!({
        "label": "write the report",
        "delegation": {"spec": {"role": "worker"}, "accept": {"command": "true"}}
    });
    open_plan(&engine, serde_json::json!([delegated]))?;
    apply(
        Actor::Child(AgentId::new("child-0")?),
        Op::Submit {
            label: job.clone(),
            attempt: AttemptId::FIRST,
            output: "local://out/report.json".parse()?,
        },
    )?;
    apply(
        Actor::Owner,
        Op::Block {
            label: job.clone(),
            on: BlockedOn::User,
            note: "needs a decision".to_owned(),
            ask: None,
        },
    )?;
    let refused = apply(
        Actor::Owner,
        Op::Done {
            label: job.clone(),
            output: None,
        },
    );
    assert!(refused.is_err(), "a blocked todo cannot be done");
    apply(
        Actor::User("user://3".parse()?),
        Op::Unblock {
            label: job.clone(),
            answer: None,
        },
    )?;

    let mut found = Scan::default();
    fold_journals(&mut found, &dir, 0);
    let tally: Vec<_> = found
        .plan_ops
        .iter()
        .map(|(kind, count)| (kind.0.as_str(), kind.1, *count))
        .collect();
    assert_eq!(
        tally,
        [
            ("block", ActorClass::Owner, 1),
            ("done (rejected)", ActorClass::Owner, 1),
            ("init", ActorClass::Owner, 1),
            ("spawn_intent", ActorClass::Engine, 2),
            ("spawn_result", ActorClass::Engine, 2),
            ("start", ActorClass::Engine, 2),
            ("submit", ActorClass::Child, 1),
            ("unblock", ActorClass::Other, 1),
        ]
    );
    assert_eq!(
        (found.journals, found.bad_lines, found.unreadable),
        (1, 0, 0)
    );
    Ok(())
}

/// Dies when `--since` keeps a plan op stamped before the window because its journal was touched
/// after it, or drops one stamped inside. The engine's clock is pinned; the file's mtime is now.
#[test]
fn a_window_keeps_plan_ops_by_their_own_stamp() -> TestResult {
    use yi_runtime::plan::ops::PlanEngine;
    use yi_runtime::plan::store::PlanStore;
    use yi_runtime::rollup::{Scan, fold_journals};
    let dir = Scratch::new("yi-rollup-journal-window")?;
    let store = PlanStore::open(dir.to_path_buf())?.with_clock(Arc::new(|| 5_000));
    open_plan(
        &PlanEngine::new(store, Arc::new(crate::plan_ops::Stub::default())),
        serde_json::json!([{"label": "cut"}]),
    )?;
    for (since, ops, journals) in [(5_000, 1, 1), (5_001, 0, 1), (u64::MAX, 0, 0)] {
        let mut found = Scan::default();
        fold_journals(&mut found, &dir, since);
        assert_eq!(
            (found.plan_ops.values().sum::<u64>(), found.journals),
            (ops, journals),
            "since {since}"
        );
    }
    Ok(())
}

/// Dies when the tally reads a journal line the journal's own reader calls damage: a copy of a
/// real record padded to the record cap counts, one byte past it is a bad line and not an op.
#[test]
fn a_journal_line_past_the_record_cap_is_a_bad_line_not_an_op() -> TestResult {
    use yi_runtime::plan::journal::RECORD_CAP;
    use yi_runtime::plan::ops::PlanEngine;
    use yi_runtime::plan::store::{JOURNAL_NAME, PlanStore};
    use yi_runtime::rollup::{Scan, fold_journals};
    for (extra, ops, bad) in [(0, 2, 0), (1, 1, 1)] {
        let dir = Scratch::new("yi-rollup-journal-cap")?;
        open_plan(
            &PlanEngine::new(
                PlanStore::open(dir.to_path_buf())?,
                Arc::new(crate::plan_ops::Stub::default()),
            ),
            serde_json::json!([{"label": "pad"}]),
        )?;
        let journal = std::fs::read_dir(&*dir)?
            .flatten()
            .map(|entry| entry.path().join(JOURNAL_NAME))
            .find(|path| path.is_file())
            .ok_or("the engine wrote no journal")?;
        let text = std::fs::read_to_string(&journal)?;
        let record = text
            .lines()
            .next()
            .and_then(|line| line.strip_suffix('}'))
            .ok_or("the journal holds no record")?;
        let pad = (RECORD_CAP + extra)
            .checked_sub(record.len() + r#","pad":""}"#.len() + 1)
            .ok_or("the record is already over the cap")?;
        let padded = format!("{record},\"pad\":\"{}\"}}\n", "x".repeat(pad));
        assert_eq!(padded.len(), RECORD_CAP + extra);
        std::fs::write(&journal, format!("{text}{padded}"))?;
        let mut found = Scan::default();
        fold_journals(&mut found, &dir, 0);
        assert_eq!(
            (found.plan_ops.values().sum::<u64>(), found.bad_lines),
            (ops, bad),
            "a line of {} bytes",
            padded.len()
        );
    }
    Ok(())
}
