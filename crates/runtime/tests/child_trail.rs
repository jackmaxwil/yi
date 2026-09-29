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
fn host(
    depth: u8,
    parent_dir: PathBuf,
    parent: Option<yi_session::SharedSession>,
) -> Arc<SubagentHost> {
    let (events, _keep) = tokio::sync::broadcast::channel(64);
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
        factory: Arc::new(|build: yi_runtime::ChildBuild<'_>| {
            let provider = Arc::new(ProviderStream::new(None));
            provider.queue_faux(vec![faux_assistant_message(
                vec![faux_text("the token is minted in auth.rs")],
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
        store: Arc::new(move || parent.clone()),
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
    for _ in 0..400 {
        let lines = trail(root)?;
        if lines.iter().any(|line| line["event"] == "ended") {
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
    let host = host(0, scratch.join("rlm-1"), Some(session));
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
    let first = host(0, scratch.join("rlm-1"), Some(Arc::clone(&session)));
    first.spawn("Where is the token minted?".to_owned(), kwargs("scout"))?;
    ended(&root_file).await?;
    drop(first);
    let reopened = yi_session::load_session(&root_file)?;
    let after = host(
        0,
        scratch.join("rlm-2"),
        Some(Arc::new(std::sync::Mutex::new(reopened))),
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
    let host = host(1, child_dir.clone(), None);
    let reply = host.spawn("Probe.".to_owned(), kwargs("probe"))?;
    let grandchild = PathBuf::from(reply["session_dir"].as_str().ok_or("no session_dir")?);
    assert_eq!(
        grandchild.parent(),
        Some(child_dir.join("children").as_path())
    );
    Ok(())
}
