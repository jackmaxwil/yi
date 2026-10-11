use crate::scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_runtime::Wall;
use yi_runtime::fetch::{Resolver, SessionTranscripts};
use yi_session::SessionRepo;
use yi_types::message::{AgentMessage, Content};

type TestResult = Result<(), Box<dyn Error>>;

fn tool_text(
    resolver: Resolver,
    name: &str,
    input: Value,
) -> Result<(String, bool), Box<dyn Error>> {
    crate::support::routed_tool(&Arc::new(resolver), name, input)
}

fn bash_result(text: String) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: "t1".to_owned(),
        tool_name: "bash".to_owned(),
        content: vec![Content::Text {
            text,
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    }
}

struct Family {
    root: Scratch,
    cwd: std::path::PathBuf,
    transcripts: Arc<SessionTranscripts>,
    host: Arc<yi_runtime::SubagentHost>,
    parent: yi_session::SharedSession,
    child: yi_session::SharedSession,
    earlier: String,
}

fn family() -> Result<Family, Box<dyn Error>> {
    let root = Scratch::new("yi-fetch-grep")?;
    let (sessions, cwd) = (root.join("sessions"), root.join("repo"));
    std::fs::create_dir_all(&cwd)?;
    let cwd = cwd.canonicalize()?;
    let mut repo = yi_session::JsonlRepo::new(sessions.clone(), cwd.to_string_lossy());
    let earlier = repo.create(yi_session::CreateOptions::default())?;
    yi_session::lock_session(&earlier)
        .append_message("main", bash_result(crate::support::report_output()))?;
    let earlier = yi_session::lock_session(&earlier).metadata().id.clone();
    let parent = repo.create(yi_session::CreateOptions::default())?;
    let parent_id = yi_session::lock_session(&parent).metadata().id.clone();
    let asked = AgentMessage::User {
        content: yi_types::message::UserContent::Text(
            "what release_id did the earlier run print?".to_owned(),
        ),
        timestamp: 0,
        attribution: yi_types::message::Attribution::User,
    };
    yi_session::lock_session(&parent).append_message("main", asked)?;
    yi_session::lock_session(&parent)
        .append_message("main", bash_result(crate::support::report_output()))?;
    let mut spawn = serde_json::Map::new();
    spawn.insert(
        "code".to_owned(),
        "rlm.run('What release_id = line did it print?')".into(),
    );
    let spawn = yi_ai::faux::faux_assistant_message(
        vec![yi_ai::faux::faux_tool_call("c1", "ipython", spawn)],
        yi_types::message::StopReason::ToolUse,
    );
    yi_session::lock_session(&parent).append_message("main", spawn)?;
    let child = repo.create(yi_session::CreateOptions {
        parent_session_id: Some(parent_id),
        ..yi_session::CreateOptions::default()
    })?;
    let host = crate::support::family(root.to_path_buf(), cwd.clone(), child.clone(), None).host;
    let transcripts = Arc::new(SessionTranscripts::new(host.clone(), Some(sessions), &cwd));
    Ok(Family {
        root,
        cwd,
        transcripts,
        host,
        parent,
        child,
        earlier,
    })
}

fn resolver(family: &Family, store: &yi_session::SharedSession, wall: Wall) -> Resolver {
    Resolver::new(family.cwd.clone(), wall)
        .with_session("worker", store.clone())
        .with_transcripts(family.transcripts.clone())
}

fn hit_address(text: &str, needle: &str) -> Result<String, Box<dyn Error>> {
    let row = text
        .lines()
        .find(|line| line.contains(needle))
        .ok_or(format!("no row with {needle}: {text}"))?;
    Ok(row.split(' ').next().unwrap_or_default().to_owned())
}

/// The incident's task: a fact on line 219 of a parent's tool result, which the compact view
/// drops, is one grep away, as a row whose address `read` opens; every role gets it or a refusal.
#[test]
fn a_child_greps_its_parents_tool_output_by_alias() -> TestResult {
    let family = family()?;
    let ask = json!({"pattern": "release_id =", "path": "history://parent"});
    let (text, is_error) = tool_text(
        resolver(&family, &family.child, Wall::default()),
        "grep",
        ask.clone(),
    )?;
    assert!(!is_error, "{text}");
    assert_eq!(text.lines().count(), 2, "{text}");
    assert!(
        text.lines()
            .next()
            .is_some_and(|line| line.ends_with(" result bash: release_id = rel-09d88875")),
        "{text}"
    );
    assert!(
        text.lines()
            .nth(1)
            .is_some_and(|line| line.contains(" call ipython: ")),
        "{text}"
    );
    let address = hit_address(&text, "release_id")?;
    assert!(address.starts_with("history://parent/"), "{address}");
    for (window, rows) in [("since/0", 2), ("tail/1", 1)] {
        let windowed =
            json!({"pattern": "release_id =", "path": format!("history://parent/{window}")});
        let (got, is_error) = tool_text(
            resolver(&family, &family.child, Wall::default()),
            "grep",
            windowed,
        )?;
        assert!(!is_error && got.lines().count() == rows, "{window}: {got}");
        assert!(
            got.lines()
                .all(|line| line.starts_with("history://parent/")),
            "{window}: {got}"
        );
    }
    let (entry, _) = tool_text(
        resolver(&family, &family.child, Wall::default()),
        "read",
        json!({"path": address}),
    )?;
    assert!(entry.contains("release_id = rel-09d88875"), "{entry}");

    let writes = Wall {
        deny_write: vec![family.cwd.clone()],
        ..Wall::default()
    };
    let (text, is_error) = tool_text(
        resolver(&family, &family.child, writes),
        "grep",
        ask.clone(),
    )?;
    assert!(!is_error && text.contains("rel-09d88875"), "{text}");
    let history = Wall {
        deny_url: vec!["history://main".to_owned()],
        ..Wall::default()
    };
    let (text, is_error) = tool_text(
        resolver(&family, &family.child, history),
        "grep",
        ask.clone(),
    )?;
    assert!(is_error && text.contains("denied"), "{text}");
    let (text, is_error) = tool_text(
        resolver(&family, &family.parent, Wall::default()),
        "grep",
        ask,
    )?;
    assert!(is_error && text.contains("grep's history://repo"), "{text}");
    Ok(())
}

/// A past session's tool output, which `compact.search` never indexes, is found under
/// `history://repo`; the searching session's own transcript, which names the pattern, is not.
#[test]
fn history_repo_greps_this_repositorys_other_sessions() -> TestResult {
    let family = family()?;
    let gone = family.root.join("gone").to_string_lossy().into_owned();
    yi_session::JsonlRepo::new(family.root.join("sessions"), gone)
        .create(yi_session::CreateOptions::default())?;
    let ask = json!({"pattern": "release_id", "path": "history://repo"});
    let (text, is_error) = tool_text(
        resolver(&family, &family.parent, Wall::default()),
        "grep",
        ask.clone(),
    )?;
    assert!(!is_error, "{text}");
    assert_eq!(
        text.lines().last(),
        Some(
            "[… 1 not searched: session directories whose checkout is gone, and unreadable session files]"
        )
    );
    let rows: Vec<&str> = text
        .lines()
        .filter(|line| line.contains("release_id"))
        .collect();
    assert!(
        rows.iter().all(|row| !row.contains("what release_id")),
        "{text}"
    );
    let address = hit_address(&text, "rel-09d88875")?;
    assert!(address.contains(&family.earlier), "{text}");
    let (entry, _) = tool_text(
        resolver(&family, &family.parent, Wall::default()),
        "read",
        json!({"path": address}),
    )?;
    assert!(entry.contains("release_id = rel-09d88875"), "{entry}");
    let history = Wall {
        deny_url: vec!["history://main".to_owned()],
        ..Wall::default()
    };
    let (text, is_error) = tool_text(
        resolver(&family, &family.parent, history),
        "grep",
        ask.clone(),
    )?;
    assert!(is_error && text.contains("denied"), "{text}");
    let reader = Resolver::new(family.cwd.clone(), Wall::default())
        .with_session("worker", family.child.clone())
        .with_transcripts(Arc::new(yi_runtime::fetch::ParentTranscript(
            Arc::downgrade(&family.host),
        )));
    let (text, is_error) = tool_text(reader, "grep", ask)?;
    assert!(is_error && text.contains("history://repo does"), "{text}");
    Ok(())
}

/// The page cap at its limit and one past, a hit line at 200 chars and one past on a non-ASCII
/// char, a zero-hit search and a file-only option, each named in the model's own view.
#[test]
fn a_history_grep_names_every_cut_it_makes() -> TestResult {
    let family = family()?;
    let store = &family.child;
    let page = |count: usize, offset: usize| -> Result<String, Box<dyn Error>> {
        let text = (0..count)
            .map(|i| format!("hit {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let (sessions, cwd) = (family.cwd.join(format!("s{count}")), family.cwd.clone());
        let mut repo = yi_session::JsonlRepo::new(sessions, cwd.to_string_lossy());
        let session = repo.create(yi_session::CreateOptions::default())?;
        yi_session::lock_session(&session).append_message("main", bash_result(text))?;
        let resolver = Resolver::new(cwd, Wall::default()).with_session("main", session);
        Ok(tool_text(
            resolver,
            "grep",
            json!({"pattern": "^hit", "path": "history://self", "offset": offset}),
        )?
        .0)
    };
    let full = page(40, 0)?;
    assert_eq!(full.lines().count(), 40, "{full}");
    let over = page(41, 0)?;
    assert_eq!(
        over.lines().last(),
        Some(
            "[… hits 1-40 of 41 in 1 entries of 1 sessions · history grep page 40 · grep again with offset=40]"
        )
    );
    let rest = page(41, 40)?;
    assert!(rest.ends_with("result bash: hit 40"), "{rest}");

    let long = |chars: usize| -> Result<String, Box<dyn Error>> {
        let line = format!("needle {}", "é".repeat(chars - 7));
        let session = Arc::new(std::sync::Mutex::new(yi_session::SessionStore::in_memory(
            yi_session::lock_session(store).metadata().clone(),
        )));
        yi_session::lock_session(&session).append_message("main", bash_result(line))?;
        let resolver =
            Resolver::new(family.cwd.clone(), Wall::default()).with_session("main", session);
        Ok(tool_text(
            resolver,
            "grep",
            json!({"pattern": "needle", "path": "history://self"}),
        )?
        .0)
    };
    let at = long(200)?;
    assert_eq!(at.lines().count(), 1, "{at}");
    let past = long(201)?;
    assert!(
        past.lines().next().is_some_and(|line| line.ends_with("é…")),
        "{past}"
    );
    assert_eq!(
        past.lines().last(),
        Some(
            "[… a hit line is cut at 200 chars (LINE_CHARS) · read its address for the whole entry]"
        )
    );

    let ask = |input: Value| tool_text(resolver(&family, store, Wall::default()), "grep", input);
    let (none, _) = ask(json!({"pattern": "no such words", "path": "history://parent"}))?;
    assert!(
        none.starts_with("[no hits in 3 entries of 1 sessions;"),
        "{none}"
    );
    let (refused, is_error) =
        ask(json!({"pattern": "x", "path": "history://parent", "include": "*.rs"}))?;
    assert!(
        is_error && refused.starts_with("include is a file grep option"),
        "{refused}"
    );
    Ok(())
}
