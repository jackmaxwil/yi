//! Files back to before a chosen turn, by /undo's rule: what the agent moved goes back.
use crate::scratch;

use std::error::Error;
use std::sync::Arc;

use scratch::Scratch;
use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, UndoOutcome};
use yi_session::{CreateOptions, JsonlRepo, SessionRepo, SharedSession, lock_session};
use yi_types::entry::Entry;
use yi_types::message::{AgentMessage, StopReason};
use yi_types::model::{Model, ModelCost};

fn faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn write(id: &str, path: &str, content: &str) -> AgentMessage {
    let args: Map<String, Value> = match json!({"path": path, "content": content}) {
        Value::Object(args) => args,
        _ => Map::new(),
    };
    faux_assistant_message(vec![faux_tool_call(id, "write", args)], StopReason::ToolUse)
}

struct Run {
    home: Scratch,
    project: std::path::PathBuf,
    _project: Scratch,
    _sessions: Scratch,
    store: SharedSession,
    session: AgentSession,
}

/// One turn per file, each writing `<name>` = `<name>\n`, and waits for every turn end.
async fn run(files: &[&str]) -> Result<Run, Box<dyn Error>> {
    let (home, project_dir, sessions) = (
        Scratch::new("yi-undo-to-home")?,
        Scratch::new("yi-undo-to-project")?,
        Scratch::new("yi-undo-to-sessions")?,
    );
    let project = project_dir.to_path_buf();
    let mut repo = JsonlRepo::new(
        sessions.to_path_buf(),
        project.to_string_lossy().into_owned(),
    );
    let store = repo.create(CreateOptions {
        id: Some("undo-to".to_owned()),
        ..CreateOptions::default()
    })?;
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(
        files
            .iter()
            .enumerate()
            .flat_map(|(n, file)| {
                [
                    write(&format!("c{n}"), file, &format!("{file}\n")),
                    faux_assistant_message(vec![faux_text("wrote it")], StopReason::Stop),
                ]
            })
            .collect(),
    );
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.attach_store(Arc::clone(&store))?;
    session.use_tools(yi_tools::builtin_tools(), project.clone(), None);
    yi_runtime::wire_turn_checkpoints(&session, &home, &project);
    for file in files {
        session.prompt(&format!("write {file}"))?;
        session.wait_idle().await;
    }
    for _ in 0..100 {
        let ends = yi_runtime::recorded(&store)
            .iter()
            .filter(|recorded| recorded.data.at == yi_types::checkpoint::CheckpointAt::TurnEnd)
            .count();
        if ends == files.len() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    for file in files {
        assert!(project.join(file).exists(), "{file} was written");
    }
    Ok(Run {
        home,
        project,
        _project: project_dir,
        _sessions: sessions,
        store,
        session,
    })
}

fn prompts(store: &SharedSession) -> Result<Vec<String>, Box<dyn Error>> {
    Ok(lock_session(store)
        .find_entries(&yi_session::EntryQuery {
            order: yi_session::EntryOrder::OldestFirst,
            ..yi_session::EntryQuery::default()
        })?
        .into_iter()
        .filter_map(|entry| match &entry {
            Entry::Message {
                message: AgentMessage::User { .. },
                id,
                ..
            } => Some(id.clone()),
            _ => None,
        })
        .collect())
}

fn restore_and_rewind(run: &Run, entry: &str) -> Result<Vec<yi_tools::Change>, Box<dyn Error>> {
    let rewind = || yi_runtime::rewind_to(&run.session, entry).map(drop);
    match yi_runtime::undo_to(&run.store, entry, &run.project, &run.home, rewind) {
        UndoOutcome::Restored { changes, .. } => Ok(changes),
        UndoOutcome::NoCheckpoint => Err("no checkpoint".into()),
        UndoOutcome::Failed(error) => Err(error.into()),
    }
}

/// Dies with a rewind that left the files where the later turns put them: the conversation
/// went back two turns and the work it no longer knew about stayed on disk.
#[tokio::test]
async fn a_restore_to_an_earlier_turn_moves_back_what_the_agent_wrote_and_keeps_hand_edits()
-> Result<(), Box<dyn Error>> {
    let run = run(&["a.txt", "b.txt"]).await?;
    std::fs::write(run.project.join("mine.txt"), "hand\n")?;
    let first = prompts(&run.store)?
        .into_iter()
        .next()
        .ok_or("no first turn")?;
    let changes = restore_and_rewind(&run, &first)?;
    assert!(!changes.is_empty());
    assert!(
        !run.project.join("a.txt").exists(),
        "turn one's file went back"
    );
    assert!(
        !run.project.join("b.txt").exists(),
        "turn two's file went back"
    );
    assert_eq!(
        std::fs::read_to_string(run.project.join("mine.txt"))?,
        "hand\n"
    );
    Ok(())
}

/// Dies with a `/undo` after a rewind's file restore that finds no record of it and reverts
/// the turn before as well, deleting work the restore never touched.
#[tokio::test]
async fn an_undo_after_a_restore_puts_back_what_the_restore_moved_and_nothing_else()
-> Result<(), Box<dyn Error>> {
    let run = run(&["a.txt", "b.txt"]).await?;
    let second = prompts(&run.store)?
        .get(1)
        .cloned()
        .ok_or("no second turn")?;
    restore_and_rewind(&run, &second)?;
    assert!(
        !run.project.join("b.txt").exists(),
        "turn two's file went back"
    );
    let UndoOutcome::Restored { .. } = yi_runtime::undo(&run.store, &run.project, &run.home) else {
        return Err("no redo".into());
    };
    let read = |file: &str| std::fs::read_to_string(run.project.join(file)).ok();
    assert_eq!(
        read("a.txt").as_deref(),
        Some("a.txt\n"),
        "turn one's file untouched"
    );
    assert_eq!(
        read("b.txt").as_deref(),
        Some("b.txt\n"),
        "the restore redone"
    );
    Ok(())
}

/// Dies with a restore after a conversation-only rewind that pairs with the rewound branch's
/// turn end: the later turns' files stay on disk and no note names them.
#[tokio::test]
async fn a_restore_after_a_conversation_rewind_also_moves_back_the_rewound_turns_files()
-> Result<(), Box<dyn Error>> {
    let run = run(&["a.txt", "b.txt", "c.txt"]).await?;
    let turns = prompts(&run.store)?;
    let third = turns.get(2).ok_or("no third turn")?;
    yi_runtime::rewind_to(&run.session, third)?;
    let second = turns.get(1).ok_or("no second turn")?;
    let changes = restore_and_rewind(&run, second)?;
    assert!(
        changes
            .iter()
            .all(|change| change.kind != yi_tools::ChangeKind::Kept),
        "the rewound turn's file is the agent's, not the user's: {changes:?}"
    );
    assert!(
        !run.project.join("b.txt").exists(),
        "turn two's file went back"
    );
    assert!(
        !run.project.join("c.txt").exists(),
        "turn three's file went back"
    );
    assert!(run.project.join("a.txt").exists(), "turn one's file stays");
    Ok(())
}

type TestResult = Result<(), Box<dyn Error>>;

fn snapshots(
    store: &SharedSession,
) -> Result<Vec<yi_types::checkpoint::TreeSnapshot>, Box<dyn Error>> {
    Ok(lock_session(store).custom_records(yi_session::EntryOrder::OldestFirst, None))
}

/// Dies with the snapshot recorded as a checkpoint: `/undo` would land mid-turn, on the last call
/// instead of where the turn began.
#[tokio::test]
async fn ten_calls_leave_snapshots_and_undo_still_goes_to_the_turn_start() -> TestResult {
    let files = ["a", "b", "c", "d", "e", "f", "g", "h", "i", "j"];
    let run = run(&files).await?;
    for _ in 0..200 {
        if !snapshots(&run.store)?.is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let taken = snapshots(&run.store)?;
    assert!(!taken.is_empty(), "no call was captured");
    assert!(taken.iter().any(|one| one.call.is_some()), "{taken:?}");
    yi_runtime::undo(&run.store, &run.project, &run.home);
    assert!(run.project.join("i").exists(), "the turn before stands");
    assert!(
        !run.project.join("j").exists(),
        "the last turn went back whole"
    );
    Ok(())
}

#[test]
fn a_capture_at_its_budget_stays_on_and_one_past_turns_off() {
    let budget = std::time::Duration::from_millis(500);
    assert!(!yi_runtime::over_budget(budget, budget));
    assert!(yi_runtime::over_budget(
        budget + std::time::Duration::from_millis(1),
        budget
    ));
}

/// Dies with the guard off: on a tree too slow for it, every tool call would load the machine with
/// another background git. The notice is what `t.log` prints at the cut.
#[tokio::test]
async fn a_tree_too_slow_to_capture_stops_trying_and_says_so() -> TestResult {
    use std::os::unix::fs::PermissionsExt;
    let shim = Scratch::new("yi-undo-to-shim")?;
    let path = std::env::var_os("PATH").ok_or("no PATH")?;
    let real = (std::env::split_paths(&path).map(|dir| dir.join("git")))
        .find(|candidate| candidate.is_file())
        .ok_or("no git on PATH")?;
    let body = format!(
        "#!/bin/sh\ncase \"$*\" in *write-tree*) sleep 1;; esac\nexec '{}' \"$@\"\n",
        real.display()
    );
    std::fs::write(shim.join("git"), body)?;
    std::fs::set_permissions(shim.join("git"), std::fs::Permissions::from_mode(0o755))?;
    let joined = std::env::join_paths(
        std::iter::once(shim.to_path_buf()).chain(std::env::split_paths(&path)),
    )?;
    // nextest gives each test its own process.
    unsafe { std::env::set_var("PATH", joined) };
    let run = run(&["a", "b"]).await?;
    let mut row = None;
    for _ in 0..300 {
        row = yi_runtime::snapshots_off_row(&run.project);
        if row.is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let row = row.ok_or("snapshots stayed on")?;
    assert!(
        row.starts_with("[… per-call snapshots off: last capture "),
        "{row}"
    );
    assert!(
        row.contains("(snapshot.budget_ms 500); t.head() still works]"),
        "{row}"
    );
    Ok(())
}

/// Dies with `restore` asked for a tree its store lacks: a bad-object error on the way to a rewind.
#[tokio::test]
async fn an_undo_whose_turn_end_is_in_another_store_moves_nothing() -> Result<(), Box<dyn Error>> {
    let run = run(&["a"]).await?;
    let missing = "0123456789abcdef0123456789abcdef01234567";
    let data = yi_types::checkpoint::CheckpointData {
        tree: missing.to_owned(),
        at: yi_types::checkpoint::CheckpointAt::TurnEnd,
        after: None,
    };
    lock_session(&run.store).append_custom_record(&data)?;
    let outcome = yi_runtime::undo(&run.store, &run.project, &run.home);
    assert!(matches!(outcome, yi_runtime::UndoOutcome::NoCheckpoint));
    assert!(run.project.join("a").exists(), "the tree stands");
    Ok(())
}
