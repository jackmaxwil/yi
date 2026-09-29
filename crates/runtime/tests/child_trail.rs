use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, SubagentHost, SubagentHostOptions};
use yi_types::message::StopReason;
use yi_types::model::{Effort, Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

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

/// A host whose parent has a real transcript on disk, as every CLI root does at spawn time.
type Slot = Arc<std::sync::Mutex<Option<yi_session::SharedSession>>>;

fn slot(parent: Option<yi_session::SharedSession>) -> Slot {
    Arc::new(std::sync::Mutex::new(parent))
}

fn host(depth: u8, parent_dir: PathBuf, parent: Slot) -> Arc<SubagentHost> {
    let (events, _keep) = tokio::sync::broadcast::channel(64);
    let spawned = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    Arc::new(SubagentHost::new(SubagentHostOptions {
        provider: Arc::new(ProviderStream::new(None)),
        depth,
        max_depth: depth.saturating_add(1),
        max_children: 4,
        parent_session_dir: parent_dir,
        cwd: std::env::temp_dir(),
        home: std::env::temp_dir(),
        lane_slots: 1,
        defaults: Arc::new(|| (faux_model(), Effort::Medium)),
        factory: Arc::new(move |build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None));
            let n = spawned.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            let answer = format!("the token is minted in auth.rs, answer {n}");
            provider.queue_faux(vec![faux_assistant_message(
                vec![faux_text(&answer)],
                StopReason::Stop,
            )]);
            Ok(AgentSession::new(
                SessionConfig {
                    system_prompt: "child sys".to_owned(),
                    model: build.model,
                    thinking_level: build.thinking,
                    tool_execution: ExecutionMode::Sequential,
                },
                provider,
            ))
        }),
        notice: Arc::new(|_, _| {}),
        events,
        report: Arc::new(|_, _| {}),
        parent_messages: Arc::new(Vec::new),
        attribute: Arc::new(|_| {}),
        store: Arc::new(move || parent.lock().ok().and_then(|held| held.clone())),
        plans_dir: std::env::temp_dir().join(".yi/plans"),
        family_live: Arc::new(|| 0),
    }))
}

fn root_session(
    dir: &Path,
) -> Result<(yi_session::SharedSession, PathBuf, String), Box<dyn Error>> {
    let mut repo = yi_session::JsonlRepo::new(dir.join("sessions"), "/work");
    let session = yi_session::SessionRepo::create(&mut repo, yi_session::CreateOptions::default())?;
    let (file, id) = {
        let store = yi_session::lock_session(&session);
        let file = store.file_path().cloned().ok_or("no root file")?;
        (file, store.metadata().id.clone())
    };
    Ok((session, file, id))
}

fn kwargs(name: &str) -> Map<String, Value> {
    json!({"name": name, "role": "root"})
        .as_object()
        .cloned()
        .unwrap_or_default()
}

fn lines(path: &Path) -> Result<Vec<Value>, Box<dyn Error>> {
    let text = std::fs::read_to_string(path)?;
    Ok(text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect())
}

fn trail(root: &Path) -> Result<Vec<Value>, Box<dyn Error>> {
    Ok(lines(root)?
        .into_iter()
        .filter(|line| line["customType"] == "child")
        .map(|line| line["data"].clone())
        .collect())
}

async fn ended(root: &Path) -> Result<Vec<Value>, Box<dyn Error>> {
    ended_n(root, 1).await
}

async fn ended_n(root: &Path, n: usize) -> Result<Vec<Value>, Box<dyn Error>> {
    for _ in 0..400 {
        let lines = trail(root)?;
        if lines.iter().filter(|line| line["event"] == "ended").count() >= n {
            return Ok(lines);
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    Err("the child's end was never journaled".into())
}

/// Dies with the old layout: the child's directory sits in `rlm-<pid>/`, beside every other
/// session's children, its header names no parent, and nothing in the root says where it went.
#[tokio::test]
async fn a_child_lives_inside_its_parent_and_the_parent_points_at_it() -> TestResult {
    let scratch = Scratch::new("yi-child-trail")?;
    let (session, root_file, root_id) = root_session(&scratch)?;
    let host = host(0, scratch.join("rlm-1"), slot(Some(session)));
    let reply = host.spawn("Where is the token minted?".to_owned(), kwargs("scout"))?;
    let child_dir = PathBuf::from(reply["session_dir"].as_str().ok_or("no session_dir")?);
    assert_eq!(
        child_dir.parent(),
        Some(root_file.with_extension("").join("children").as_path())
    );
    let lines = ended(&root_file).await?;
    let spawned = lines
        .iter()
        .find(|line| line["event"] == "spawned")
        .ok_or("no spawn line")?;
    assert_eq!(spawned["name"], "scout");
    assert_eq!(spawned["id"], reply["rlm_child_id"]);
    let child_file = root_file
        .parent()
        .ok_or("root file has no directory")?
        .join(spawned["path"].as_str().ok_or("no path")?);
    assert!(
        child_file.starts_with(&child_dir),
        "{}",
        child_file.display()
    );
    let header = lines_of_header(&child_file)?;
    assert_eq!(header["parentSessionId"], root_id.as_str());
    assert_eq!(header["id"], spawned["session"]);
    let end = lines
        .iter()
        .find(|line| line["event"] == "ended")
        .ok_or("no end line")?;
    assert_eq!(end["exit"]["kind"], "completed");
    assert_eq!(end["name"], "scout");
    Ok(())
}

fn lines_of_header(path: &Path) -> Result<Value, Box<dyn Error>> {
    lines(path)?
        .into_iter()
        .next()
        .ok_or_else(|| "empty transcript".into())
}

/// Dies with `history://` reading only the live roster and the in-memory reaped map: a host
/// built over the same root after a restart cannot find the child it spawned before.
#[tokio::test]
async fn a_childs_history_resolves_through_the_trail_after_a_restart() -> TestResult {
    let scratch = Scratch::new("yi-child-trail-restart")?;
    let (session, root_file, _) = root_session(&scratch)?;
    let first = host(0, scratch.join("rlm-1"), slot(Some(Arc::clone(&session))));
    first.spawn("Where is the token minted?".to_owned(), kwargs("scout"))?;
    ended(&root_file).await?;
    drop(first);
    let reopened = yi_session::load_session(&root_file)?;
    let after = host(
        0,
        scratch.join("rlm-2"),
        slot(Some(Arc::new(std::sync::Mutex::new(reopened)))),
    );
    let resolver =
        yi_runtime::fetch::Resolver::new(std::env::temp_dir(), yi_runtime::Wall::default())
            .with_transcripts(Arc::new(yi_runtime::fetch::SessionTranscripts::new(
                after,
                None,
                Path::new("/work"),
            )));
    let fetched = resolver.fetch(&"history://scout".parse()?)?;
    assert!(
        fetched.text.contains("minted in auth.rs"),
        "{}",
        fetched.text
    );
    Ok(())
}

/// A child's own children nest the same way, under the directory the child already owns.
#[tokio::test]
async fn a_grandchild_nests_under_its_parents_directory() -> TestResult {
    let scratch = Scratch::new("yi-child-trail-nested")?;
    let child_dir = scratch.join("children/sub-00000001");
    let host = host(1, child_dir.clone(), slot(None));
    let reply = host.spawn("Probe.".to_owned(), kwargs("probe"))?;
    let grandchild = PathBuf::from(reply["session_dir"].as_str().ok_or("no session_dir")?);
    assert_eq!(
        grandchild.parent(),
        Some(child_dir.join("children").as_path())
    );
    Ok(())
}

fn history(host: Arc<SubagentHost>, agent: &str) -> Result<String, Box<dyn Error>> {
    let resolver =
        yi_runtime::fetch::Resolver::new(std::env::temp_dir(), yi_runtime::Wall::default())
            .with_transcripts(Arc::new(yi_runtime::fetch::SessionTranscripts::new(
                host,
                None,
                Path::new("/work"),
            )));
    Ok(resolver.fetch(&format!("history://{agent}").parse()?)?.text)
}

/// Dies with the trail walked oldest first: a name reused after a reap reads the first child's
/// transcript after a restart, while the live process read the second.
#[tokio::test]
async fn a_reused_name_resolves_to_the_newest_child_and_an_id_to_its_own() -> TestResult {
    let scratch = Scratch::new("yi-child-trail-reuse")?;
    let (session, root_file, _) = root_session(&scratch)?;
    let first = host(0, scratch.join("rlm-1"), slot(Some(Arc::clone(&session))));
    let old = first.spawn("One.".to_owned(), kwargs("scout"))?;
    ended_n(&root_file, 1).await?;
    first.delete("scout")?;
    first.spawn("Two.".to_owned(), kwargs("scout"))?;
    ended_n(&root_file, 2).await?;
    drop(first);
    let reopened = Arc::new(std::sync::Mutex::new(yi_session::load_session(&root_file)?));
    let after = host(0, scratch.join("rlm-2"), slot(Some(reopened)));
    let newest = history(Arc::clone(&after), "scout")?;
    assert!(newest.contains("answer 1"), "{newest}");
    let by_id = history(after, old["rlm_child_id"].as_str().ok_or("no id")?)?;
    assert!(by_id.contains("answer 0"), "{by_id}");
    Ok(())
}

async fn settled(host: &Arc<SubagentHost>) -> TestResult {
    for _ in 0..400 {
        let list = host.list();
        let done = list["subagents"]
            .as_array()
            .is_some_and(|all| all.iter().all(|entry| entry["status"] != "running"));
        if done {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    }
    Err("the child never settled".into())
}

/// Dies with the end written to whatever the live handle holds: after `/new` swaps the root's
/// transcript, the child's end lands in a session that never spawned it.
#[tokio::test]
async fn a_childs_end_goes_nowhere_once_its_parent_is_swapped_away() -> TestResult {
    let scratch = Scratch::new("yi-child-trail-swap")?;
    let (session, root_file, _) = root_session(&scratch)?;
    let (next, next_file, _) = root_session(&scratch)?;
    let parent = slot(Some(session));
    let host = host(0, scratch.join("rlm-1"), Arc::clone(&parent));
    host.spawn("Where is the token minted?".to_owned(), kwargs("scout"))?;
    *parent.lock().map_err(|_| "poisoned")? = Some(next);
    settled(&host).await?;
    assert!(trail(&next_file)?.is_empty(), "{:?}", trail(&next_file)?);
    let old = trail(&root_file)?;
    assert!(old.iter().all(|line| line["event"] == "spawned"), "{old:?}");
    Ok(())
}

/// Dies with the end written through a copy kept at spawn: rpc's `switch_session` reloads the
/// same file as a second copy, the parent appends through it, and the child's end then reused
/// that sequence number, so the file no longer loaded.
#[tokio::test]
async fn a_reloaded_parent_takes_the_childs_end_without_breaking_its_file() -> TestResult {
    let scratch = Scratch::new("yi-child-trail-reload")?;
    let (session, root_file, _) = root_session(&scratch)?;
    let parent = slot(Some(session));
    let host = host(0, scratch.join("rlm-1"), Arc::clone(&parent));
    host.spawn("Where is the token minted?".to_owned(), kwargs("scout"))?;
    let reloaded = Arc::new(std::sync::Mutex::new(yi_session::load_session(&root_file)?));
    yi_session::lock_session(&reloaded).append_custom("main", "turn", None)?;
    *parent.lock().map_err(|_| "poisoned")? = Some(reloaded);
    ended(&root_file).await?;
    let loaded = yi_session::load_session(&root_file)?;
    drop(loaded);
    Ok(())
}

/// Dies with the stem taken from any file: a transcript loaded from a path with no `.jsonl`
/// names a file, not a directory, so every spawn failed with ENOTDIR.
#[tokio::test]
async fn a_parent_file_without_the_extension_keeps_its_children_where_they_were() -> TestResult {
    let scratch = Scratch::new("yi-child-trail-noext")?;
    let (_, root_file, _) = root_session(&scratch)?;
    let bare = scratch.join("session");
    std::fs::copy(&root_file, &bare)?;
    let loaded = Arc::new(std::sync::Mutex::new(yi_session::load_session(&bare)?));
    let host = host(0, scratch.join("rlm-1"), slot(Some(loaded)));
    let reply = host.spawn("Probe.".to_owned(), kwargs("probe"))?;
    let dir = PathBuf::from(reply["session_dir"].as_str().ok_or("no session_dir")?);
    assert_eq!(dir.parent(), Some(scratch.join("rlm-1").as_path()));
    Ok(())
}
