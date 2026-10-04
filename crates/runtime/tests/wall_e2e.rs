use crate::scratch;
use scratch::Scratch;

use std::error::Error;
use std::sync::Arc;

use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_loop::ExecutionMode;
use yi_runtime::{AgentSession, ProviderStream, SessionConfig, Wall};
use yi_types::event::AgentEvent;
use yi_types::message::{Content, StopReason};
use yi_types::model::{Model, ModelCost};

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

async fn run_denied_command(wall: Wall, command: String) -> Result<(String, bool), Box<dyn Error>> {
    let mut args = serde_json::Map::new();
    args.insert("command".to_owned(), serde_json::json!(command));
    run_walled_tool(wall, "bash", args, &std::env::temp_dir()).await
}

async fn run_walled_tool(
    wall: Wall,
    tool: &str,
    args: serde_json::Map<String, serde_json::Value>,
    cwd: &std::path::Path,
) -> Result<(String, bool), Box<dyn Error>> {
    let provider = Arc::new(ProviderStream::new(None));
    provider.queue_faux(vec![
        faux_assistant_message(
            vec![faux_tool_call("call-1", tool, args)],
            StopReason::ToolUse,
        ),
        faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
    ]);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.set_wall(wall);
    session.use_tools(yi_tools::builtin_tools(), cwd.to_path_buf(), None);
    let mut events = session.subscribe();
    session.prompt("go")?;
    session.wait_idle().await;
    let mut seen = None;
    while let Ok(event) = events.try_recv() {
        if let AgentEvent::ToolExecutionEnd {
            result, is_error, ..
        } = event
        {
            let text = result
                .content
                .iter()
                .map(|content| match content {
                    Content::Text { text, .. } => text.clone(),
                    _ => String::new(),
                })
                .collect::<String>();
            seen = Some((text, is_error));
        }
    }
    seen.ok_or_else(|| "no tool result observed".into())
}

#[tokio::test]
async fn the_wall_denies_a_write_to_the_instrument_before_it_runs() -> TestResult {
    let instrument = Scratch::new("yi-wall")?;
    let marker = instrument.join("cases.json");
    let wall = Wall {
        deny_write: vec![instrument.to_path_buf()],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
        container: None,
    };

    let (denial, is_error) =
        run_denied_command(wall.clone(), format!("echo relaxed > {}", marker.display())).await?;
    assert!(is_error, "a walled path must be refused");
    assert!(
        denial.contains("Denied by the reviewer wall")
            && denial.contains(&instrument.display().to_string()),
        "the denial names the path that is off limits: {denial}"
    );
    assert!(
        !marker.exists(),
        "the command must never have run: the wall sits before execution"
    );

    let (allowed, is_error) = run_denied_command(wall, "echo untouched".to_owned()).await?;
    assert!(
        !is_error && allowed.contains("untouched"),
        "work outside the wall is untouched by it: {allowed}"
    );
    Ok(())
}

/// The orientation packet names no path in its arguments, so the wall cannot
/// refuse it from the call: the tool has to consult the deny set itself.
#[tokio::test]
async fn a_read_deny_keeps_the_orientation_packet_out_of_the_denied_tree() -> TestResult {
    let root = Scratch::new("yi-wall-orient")?;
    std::fs::create_dir_all(root.join("secret"))?;
    std::fs::write(root.join("open.rs"), "pub fn open_declaration() {}\n")?;
    std::fs::write(
        root.join("secret/hidden.rs"),
        "pub fn hidden_declaration() {}\n",
    )?;
    let wall = Wall {
        deny_write: Vec::new(),
        deny_read: vec![root.join("secret")],
        deny_url: Vec::new(),
        container: None,
    };

    let (packet, is_error) =
        run_walled_tool(wall, "get_context", serde_json::Map::new(), &root).await?;
    assert!(
        !is_error,
        "the packet still answers outside the deny: {packet}"
    );
    assert!(
        packet.contains("open_declaration"),
        "a deny narrows the packet, it does not empty it: {packet}"
    );
    assert!(
        !packet.contains("hidden_declaration"),
        "a deny_read child must not read declarations out of the denied tree: {packet}"
    );
    Ok(())
}

fn read_walled(root: &std::path::Path) -> Wall {
    Wall {
        deny_write: Vec::new(),
        deny_read: vec![root.join("secret"), root.join("vault.rs")],
        deny_url: Vec::new(),
        container: None,
    }
}

fn args(pairs: &[(&str, &str)]) -> serde_json::Map<String, serde_json::Value> {
    pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), serde_json::json!(value)))
        .collect()
}

/// grep, a read of a glob and a read of a directory name no denied path in their arguments, so
/// the wall passes them; each walk has to keep out of the denied tree on its own.
#[tokio::test]
async fn a_read_deny_keeps_every_tree_walk_out_of_the_denied_tree() -> TestResult {
    let root = Scratch::new("yi-wall-walks")?;
    std::fs::create_dir_all(root.join("secret/nested"))?;
    std::fs::write(root.join("open.rs"), "pub fn open_declaration() {}\n")?;
    std::fs::write(
        root.join("secret/nested/clé.rs"),
        "pub fn hidden_declaration() {}\n",
    )?;
    std::fs::write(root.join("vault.rs"), "pub fn vault_declaration() {}\n")?;
    let calls = [
        ("grep", args(&[("pattern", "_declaration")])),
        ("grep", args(&[("pattern", "_declaration"), ("path", ".")])),
        ("read", args(&[("path", "**/*.rs")])),
        ("read", args(&[("path", ".")])),
    ];
    for (tool, call) in calls {
        let (text, _) = run_walled_tool(read_walled(&root), tool, call.clone(), &root).await?;
        assert!(
            text.contains("open_declaration"),
            "{tool} {call:?} still answers outside the deny: {text}"
        );
        assert!(
            !text.contains("hidden_declaration") && !text.contains("vault_declaration"),
            "{tool} {call:?} read out of the denied tree: {text}"
        );
        assert!(
            text.contains("deny_read"),
            "{tool} {call:?} must say the wall cut it: {text}"
        );
    }
    Ok(())
}

/// A grep replace over the whole tree names `.` as its path, which no deny covers; the files it
/// would rewrite are what the wall has to see.
#[tokio::test]
async fn a_grep_apply_never_rewrites_a_write_denied_file() -> TestResult {
    let root = Scratch::new("yi-wall-grep-apply")?;
    std::fs::write(root.join("open.rs"), "fn old_name() {}\n")?;
    std::fs::write(root.join("check.rs"), "fn old_name() {}\n")?;
    let wall = Wall {
        deny_write: vec![root.join("check.rs")],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
        container: None,
    };
    let mut call = args(&[
        ("pattern", "old_name"),
        ("replace", "new_name"),
        ("path", "."),
    ]);
    call.insert("apply".to_owned(), serde_json::json!(true));
    let (text, is_error) = run_walled_tool(wall, "grep", call, &root).await?;
    assert!(
        is_error,
        "a replace touching a walled file is refused: {text}"
    );
    assert!(text.contains("deny_write"), "{text}");
    assert_eq!(
        std::fs::read_to_string(root.join("check.rs"))?,
        "fn old_name() {}\n",
        "the walled file is untouched"
    );
    assert_eq!(
        std::fs::read_to_string(root.join("open.rs"))?,
        "fn old_name() {}\n",
        "a refused replace writes nothing at all"
    );
    Ok(())
}

/// Dies with a private wrapper list: `timeout 5 rm` and `nice rm` named no write target, so a
/// walled file was removable by any wrapper the wall did not know.
#[test]
fn a_wrapped_write_to_a_write_denied_path_is_refused() -> TestResult {
    let root = std::env::temp_dir().join("yi-wall-wrapped");
    let check = root.join("check.py").display().to_string();
    let wall = Wall {
        deny_write: vec![root.join("check.py")],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
        container: None,
    };
    for write in [
        format!("timeout 5 rm {check}"),
        format!("nice -n 5 rm -f {check}"),
        format!("stdbuf -o0 tee {check} < /dev/null"),
        format!("ionice -c 3 touch {check}"),
        format!("FOO=1 nohup rm {check}"),
        format!("xargs rm {check}"),
    ] {
        let mut call = serde_json::Map::new();
        call.insert("command".to_owned(), serde_json::json!(write));
        let refused = wall
            .check("bash", yi_tools::ToolKind::Exec, &call, &root)
            .ok_or(format!("{write} was let through"))?;
        assert!(refused.contains("deny_write"), "{refused}");
    }
    Ok(())
}

/// Dies with the substring match: eleven confirmation `bash` reads of a `deny_write` standard
/// (`python3 /app/check.py`, `sed -n`, `ls`) were refused as if they wrote it.
#[test]
fn a_bash_read_of_a_write_denied_path_runs_and_a_write_to_it_is_refused() -> TestResult {
    let root = std::env::temp_dir().join("yi-wall-bash");
    let check = root.join("check.py").display().to_string();
    let spec = root.join("spec").display().to_string();
    let wall = Wall {
        deny_write: vec![root.join("check.py"), root.join("spec")],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
        container: None,
    };
    let bash = |command: String| {
        let mut args = serde_json::Map::new();
        args.insert("command".to_owned(), serde_json::json!(command));
        wall.check("bash", yi_tools::ToolKind::Exec, &args, &root)
    };
    for read in [
        format!("python3 {check} tablefmt; echo \"exit=$?\""),
        format!("sed -n '22p' {spec}/tablefmt.md; ls {spec} | head"),
        format!("cat {check} > /tmp/copy.py && diff {spec}/a.md /tmp/b.md"),
        format!("cp {check} /tmp/check.py"),
    ] {
        assert_eq!(bash(read.clone()), None, "{read}");
    }
    for write in [
        format!("echo x > {check}"),
        format!("sed -i 's/a/b/' {spec}/a.md"),
        format!("rm -f {check}"),
        format!("cp /tmp/x.py {check}"),
        format!("ls && tee -a {spec}/a.md < /dev/null"),
        format!("git checkout -- {check}"),
        format!("perl -pi -e 's/a/b/' {check}"),
    ] {
        let refused = bash(write.clone()).ok_or(format!("{write} was let through"))?;
        assert!(refused.contains("deny_write"), "{refused}");
    }
    Ok(())
}

#[test]
fn a_read_deny_binds_reads_and_a_write_deny_does_not() -> TestResult {
    let root = std::env::temp_dir().join("yi-wall-scope");
    let mut args = serde_json::Map::new();
    args.insert(
        "path".to_owned(),
        serde_json::json!(root.join("cases.json").display().to_string()),
    );
    let write_only = Wall {
        deny_write: vec![root.clone()],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
        container: None,
    };
    assert!(
        write_only
            .check("read", yi_tools::ToolKind::Read, &args, &root)
            .is_none(),
        "a write deny leaves reading the standard open — the reviewer still needs it"
    );
    assert!(
        write_only
            .check("write", yi_tools::ToolKind::Write, &args, &root)
            .is_some()
    );
    let read_too = Wall {
        deny_write: Vec::new(),
        deny_read: vec![root.clone()],
        deny_url: Vec::new(),
        container: None,
    };
    assert!(
        read_too
            .check("read", yi_tools::ToolKind::Read, &args, &root)
            .is_some(),
        "the sampled-instrument case opts reads in"
    );
    assert!(
        read_too
            .check("write", yi_tools::ToolKind::Write, &args, &root)
            .is_some(),
        "a path hidden from a child is not writable by it either"
    );
    Ok(())
}

#[test]
fn a_spawn_declares_url_denies_and_the_child_wall_carries_them() -> TestResult {
    let root = std::env::temp_dir().join("yi-wall-url");
    let kwargs: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"deny_url": ["kernel://", "plan://secret-cut"]}"#)?;
    let wall = Wall::from_kwargs(&kwargs, &root)?;
    assert_eq!(wall.deny_url, vec!["kernel://", "plan://secret-cut"]);
    let walled: yi_types::url::Url = "kernel://main/answers".parse()?;
    assert!(
        wall.check_url(&walled, &root).is_some(),
        "a bare scheme prefix walls the whole scheme"
    );
    let open: yi_types::url::Url = "plan://another-cut/step".parse()?;
    assert!(wall.check_url(&open, &root).is_none());
    // Incident: only `local://` mapped onto deny_read, so a walled path stayed
    // readable as of any checkpoint tree.
    let read_walled: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"deny_read": ["secret"]}"#)?;
    let wall = Wall::from_kwargs(&read_walled, &root)?;
    let tree = "a".repeat(40);
    let as_of: yi_types::url::Url = format!("checkpoint://{tree}/secret/key.txt").parse()?;
    assert!(
        wall.check_url(&as_of, &root).is_some(),
        "a read-walled path is walled as of every checkpoint tree too"
    );
    let elsewhere: yi_types::url::Url = format!("checkpoint://{tree}/src/lib.rs").parse()?;
    assert!(wall.check_url(&elsewhere, &root).is_none());
    let not_a_list: serde_json::Map<String, serde_json::Value> =
        serde_json::from_str(r#"{"deny_url": "kernel://"}"#)?;
    assert!(
        Wall::from_kwargs(&not_a_list, &root).is_err(),
        "a deny that is not a list is refused, not silently ignored"
    );
    Ok(())
}

#[tokio::test]
async fn a_grep_rewrite_is_a_write_the_wall_refuses() -> TestResult {
    let root = Scratch::new("yi-wall-grep")?;
    let file = root.join("lib.rs");
    std::fs::write(&file, "fn unsafe_thing() {}\n")?;
    let wall = Wall {
        deny_write: vec![root.to_path_buf()],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
        container: None,
    };
    for path in [None, Some("lib.rs")] {
        let mut args = serde_json::Map::new();
        args.insert("pattern".to_owned(), serde_json::json!("unsafe_thing"));
        args.insert("replace".to_owned(), serde_json::json!("safe_thing"));
        args.insert("apply".to_owned(), serde_json::json!(true));
        if let Some(path) = path {
            args.insert("path".to_owned(), serde_json::json!(path));
        }
        let (denial, is_error) = run_walled_tool(wall.clone(), "grep", args, &root).await?;
        assert!(
            is_error && denial.contains("deny_write"),
            "{path:?}: {denial}"
        );
        assert_eq!(std::fs::read_to_string(&file)?, "fn unsafe_thing() {}\n");
    }
    Ok(())
}

#[tokio::test]
async fn a_walled_subtree_holds_against_a_rewrite_and_an_env_prefix() -> TestResult {
    let root = Scratch::new("yi-wall-subtree")?;
    std::fs::create_dir_all(root.join("tests"))?;
    let file = root.join("tests/a.rs");
    std::fs::write(&file, "fn unsafe_thing() {}\n")?;
    let wall = Wall {
        deny_write: vec![root.join("tests")],
        deny_read: Vec::new(),
        deny_url: Vec::new(),
        container: None,
    };
    let mut args = serde_json::Map::new();
    args.insert("pattern".to_owned(), serde_json::json!("unsafe_thing"));
    args.insert("replace".to_owned(), serde_json::json!("safe_thing"));
    args.insert("apply".to_owned(), serde_json::json!(true));
    let (denial, is_error) = run_walled_tool(wall.clone(), "grep", args, &root).await?;
    assert!(is_error && denial.contains("deny_write"), "{denial}");
    let mut args = serde_json::Map::new();
    let command = format!("env rm -f {}", file.display());
    args.insert("command".to_owned(), serde_json::json!(command));
    let (denial, is_error) = run_walled_tool(wall, "bash", args, &root).await?;
    assert!(is_error && denial.contains("deny_write"), "{denial}");
    assert_eq!(std::fs::read_to_string(&file)?, "fn unsafe_thing() {}\n");
    Ok(())
}

/// A named path reaches a walled tree through a symlink (dangling too), a `..` after one, or
/// another letter case; each gets the refusal a missing file there gets, so the refusal says
/// nothing about what exists behind the wall. A hard link is not covered.
#[cfg(unix)]
#[test]
fn a_named_path_never_reaches_a_walled_tree_by_another_name() -> TestResult {
    let root = Scratch::new("yi-wall-names")?;
    std::fs::create_dir_all(root.join("secret/nested"))?;
    std::fs::write(root.join("secret/k.rs"), "pub fn hidden() {}\n")?;
    std::os::unix::fs::symlink(root.join("secret"), root.join("alias"))?;
    std::os::unix::fs::symlink(root.join("secret/nested"), root.join("deep"))?;
    std::os::unix::fs::symlink(root.join("secret/k.rs"), root.join("probe_present"))?;
    std::os::unix::fs::symlink("secret/missing.rs", root.join("probe_missing"))?;
    std::os::unix::fs::symlink(root.join("loop_b"), root.join("loop_a"))?;
    std::os::unix::fs::symlink(root.join("loop_a"), root.join("loop_b"))?;
    let mut paths = vec![
        "secret/k.rs",
        "alias/k.rs",
        "alias/missing.rs",
        "alias",
        "alias/*.rs",
        "deep/../k.rs",
        "deep/../missing.rs",
        "probe_present",
        "probe_missing",
    ];
    if root.join("SECRET").exists() {
        paths.extend(["SECRET/k.rs", "SECRET/missing.rs"]);
    }
    let wall = read_walled(&root);
    let refusal = |path: &str| {
        wall.check(
            "read",
            yi_tools::ToolKind::Read,
            &args(&[("path", path)]),
            &root,
        )
    };
    let expected = refusal("secret/k.rs");
    assert!(expected.is_some(), "the lexical case is refused today");
    for path in paths {
        assert_eq!(refusal(path), expected, "read {path}");
    }
    assert_eq!(refusal("open.rs"), None, "an unwalled path still reads");
    assert_eq!(
        refusal("loop_a/x.rs"),
        None,
        "a link loop ends open, as the OS's ELOOP does"
    );
    Ok(())
}

/// One turn per call, so a later call can name what an earlier one printed.
async fn run_turns(
    session: &AgentSession,
    provider: &ProviderStream,
    calls: Vec<(&str, serde_json::Map<String, serde_json::Value>)>,
) -> Result<Vec<(String, bool)>, Box<dyn Error>> {
    let mut seen = Vec::new();
    for (tool, call) in calls {
        provider.queue_faux(vec![
            faux_assistant_message(vec![faux_tool_call("c", tool, call)], StopReason::ToolUse),
            faux_assistant_message(vec![faux_text("done")], StopReason::Stop),
        ]);
        let mut events = session.subscribe();
        session.prompt("go")?;
        session.wait_idle().await;
        while let Ok(event) = events.try_recv() {
            if let AgentEvent::ToolExecutionEnd {
                result, is_error, ..
            } = event
            {
                seen.push((
                    yi_types::message::join_text(&result.content, "\n"),
                    is_error,
                ));
            }
        }
    }
    Ok(seen)
}

/// #888: spills are kept per session, and a walled session reads back its own spill but
/// neither another session's nor one from before spills were per session.
#[tokio::test]
async fn a_walled_session_reads_back_its_own_spill_and_no_other() -> TestResult {
    let root = Scratch::new("yi-wall-spill")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let other = home.join(".yi/spills/author/0123.txt");
    let legacy = home.join(".yi/tool-output/4567.txt");
    for planted in [&other, &legacy] {
        std::fs::create_dir_all(planted.parent().ok_or("no parent")?)?;
        std::fs::write(planted, "WALLED OUTPUT\n")?;
    }
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    session.attach_store(Arc::new(std::sync::Mutex::new(
        yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
            id: "juror".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        }),
    )))?;
    session.set_wall(read_walled(&root));
    session.use_tools(yi_tools::builtin_tools(), root.to_path_buf(), None);
    let cut = run_turns(
        &session,
        &provider,
        vec![("bash", args(&[("command", "seq 1 20000")]))],
    );
    let (text, _) = cut.await?.pop().ok_or("no bash result")?;
    let pointer = (text.split("[full output: ").nth(1))
        .and_then(|rest| rest.split(']').next())
        .ok_or_else(|| format!("no pointer: {text}"))?;
    assert!(
        pointer.starts_with(&home.join(".yi/spills/juror/").display().to_string()),
        "the spill is not under the session's own dir: {pointer}"
    );
    let (other_path, flat_path) = (other.display().to_string(), legacy.display().to_string());
    let mut calls = [pointer, &other_path, &flat_path]
        .map(|path| ("read", args(&[("path", path)])))
        .to_vec();
    calls.push(("grep", args(&[("pattern", "^20000$"), ("path", pointer)])));
    let seen = run_turns(&session, &provider, calls).await?;
    let [own, author, flat, grep] = seen.as_slice() else {
        return Err(format!("four calls, got {seen:?}").into());
    };
    let head: String = own.0.chars().take(300).collect();
    assert!(!own.1 && own.0.contains("\n20:20\n"), "own spill: {head}");
    assert!(
        grep.0.contains("20000"),
        "grep of its own spill: {}",
        grep.0
    );
    for (name, (text, is_error)) in [("another session's", author), ("a flat", flat)] {
        assert!(
            *is_error && !text.contains("WALLED"),
            "{name} spill reached a walled session: {text}"
        );
    }
    // An unwalled session's bash reads any file the user can, so its `read` is not walled either.
    let mut open = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    open.use_tools(yi_tools::builtin_tools(), root.to_path_buf(), None);
    let read = vec![("read", args(&[("path", other_path.as_str())]))];
    let (text, _) = run_turns(&open, &provider, read)
        .await?
        .pop()
        .ok_or("no read")?;
    assert!(text.contains("WALLED OUTPUT"), "an unwalled read: {text}");
    Ok(())
}

/// Where the transcripts of one project sit in a fake HOME's store: the author's, a sibling
/// child's under it, and the walled juror's own dir among the author's children.
pub(crate) struct Planted {
    pub(crate) project: std::path::PathBuf,
    pub(crate) sessions: std::path::PathBuf,
    pub(crate) author: std::path::PathBuf,
    pub(crate) sibling: std::path::PathBuf,
    pub(crate) own_dir: std::path::PathBuf,
    /// A child of the juror's own, in its own dir: the file is spared, never the dir.
    pub(crate) grandchild: std::path::PathBuf,
}

pub(crate) fn plant_transcripts(
    home: &std::path::Path,
    project: &std::path::Path,
) -> Result<Planted, Box<dyn Error>> {
    let project = project.to_path_buf();
    std::fs::create_dir_all(&project)?;
    let sessions = home.join(".yi/sessions");
    let family = sessions.join(yi_session::session_directory_name(
        &project.to_string_lossy(),
    ));
    let author = family.join("100_author.jsonl");
    let sibling = family.join("100_author/children/sub-sib/1_sib.jsonl");
    let own_dir = family.join("100_author/children/sub-own");
    let grandchild = own_dir.join("children/sub-g/1_g.jsonl");
    for planted in [&author, &sibling, &grandchild] {
        std::fs::create_dir_all(planted.parent().ok_or("no parent")?)?;
        std::fs::write(planted, "WALLED TRANSCRIPT\n")?;
    }
    Ok(Planted {
        project,
        sessions,
        author,
        sibling,
        own_dir,
        grandchild,
    })
}

/// The juror's shape (`plan/judge.rs`): every write walled, `history://` walled.
pub(crate) fn juror_wall(project: &std::path::Path) -> Wall {
    Wall {
        deny_write: vec![project.to_path_buf()],
        deny_read: Vec::new(),
        deny_url: vec!["history://".to_owned()],
        container: None,
    }
}

/// #971: a walled juror reads its own transcript and no other session's, under `~/.yi/sessions`
/// or the `--session-dir` in use, by any spelling; an unwalled session reads them as before.
#[tokio::test]
async fn a_walled_juror_reads_its_own_transcript_and_no_other() -> TestResult {
    let root = Scratch::new("yi-wall-transcript")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let planted = plant_transcripts(&home, &root.join("project"))?;
    let elsewhere = root.join("store/--elsewhere--/1_other.jsonl");
    std::fs::create_dir_all(elsewhere.parent().ok_or("no parent")?)?;
    std::fs::write(&elsewhere, "WALLED TRANSCRIPT\n")?;
    let project = planted.project.clone();
    std::os::unix::fs::symlink(&planted.sessions, project.join("alias"))?;
    let store = yi_session::create_flat_session(
        planted.own_dir.clone(),
        project.to_string_lossy(),
        Some("author".to_owned()),
    )?;
    let (own_id, own_file) = {
        let store = yi_session::lock_session(&store);
        let file = store.file_path().cloned().ok_or("no transcript file")?;
        (format!("\"id\":\"{}\"", store.metadata().id), file)
    };
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    session.attach_store(store)?;
    let wall = juror_wall(&project);
    session.set_wall(wall.clone());
    // The juror is a child: its broker comes from the root's, which holds the --session-dir.
    let broker = yi_runtime::PermissionBroker::new(
        yi_permission::PermissionMode::Auto,
        project.clone(),
        Vec::new(),
        None,
        session.events_sender(),
    )
    .with_session_store(&root.join("store"));
    let broker = Arc::new(broker.for_child(&wall, &project));
    session.use_tools(yi_tools::builtin_tools(), project.clone(), Some(broker));
    let (author, sessions) = (planted.author.display().to_string(), &planted.sessions);
    let name = planted.author.strip_prefix(sessions)?.display().to_string();
    let spellings = [
        author.clone(),
        author.replace("/.yi/sessions/", "/.YI/SESSIONS/"),
        planted
            .own_dir
            .join("../../../100_author.jsonl")
            .display()
            .to_string(),
        format!("alias/{name}"),
        format!("{}/*/*.jsonl", sessions.display()),
        format!("{}/**/*.jsonl", sessions.display()),
        planted.sibling.display().to_string(),
        planted.grandchild.display().to_string(),
        elsewhere.display().to_string(),
    ];
    let mut calls: Vec<_> = (spellings.iter())
        .map(|path| ("read", args(&[("path", path)])))
        .collect();
    let searched = [sessions.display().to_string(), author, "alias".to_owned()];
    calls.extend(
        (searched.iter()).map(|path| ("grep", args(&[("pattern", "WALLED"), ("path", path)]))),
    );
    let own = own_file.display().to_string();
    calls.push(("read", args(&[("path", &own)])));
    calls.push((
        "grep",
        args(&[("pattern", "kind.:.header"), ("path", &own)]),
    ));
    let mut seen = run_turns(&session, &provider, calls).await?;
    let (own_grep, own_read) = (seen.pop().ok_or("no grep")?, seen.pop().ok_or("no read")?);
    for (call, (text, _)) in spellings.iter().chain(&searched).zip(&seen) {
        assert!(
            !text.contains("WALLED TRANSCRIPT"),
            "{call} reached a walled juror: {text}"
        );
    }
    assert!(
        !own_read.1 && own_read.0.contains(&own_id),
        "own transcript: {}",
        own_read.0
    );
    assert!(
        own_grep.0.contains(&own_id),
        "grep of its own transcript: {}",
        own_grep.0
    );
    // An unwalled session's bash reads any file the user can, so its `read` is not walled either.
    let mut open = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    open.use_tools(yi_tools::builtin_tools(), project, None);
    let read = vec![(
        "read",
        args(&[("path", planted.author.display().to_string().as_str())]),
    )];
    let (text, _) = run_turns(&open, &provider, read)
        .await?
        .pop()
        .ok_or("no read")?;
    assert!(
        text.contains("WALLED TRANSCRIPT"),
        "an unwalled read: {text}"
    );
    Ok(())
}

/// #971: a walled reader's fetch reaches no transcript through a link in a member tree or in
/// its workspace, nor through a store inside the workspace (a `--session-dir` there, or cwd =
/// HOME) by `local://` or as of a checkpoint.
#[test]
fn a_walled_fetch_reaches_no_transcript_by_tree_local_or_checkpoint() -> TestResult {
    let root = Scratch::new("yi-wall-transcript-fetch")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let planted = plant_transcripts(&home, &root.join("project"))?;
    let project = planted.project.clone();
    std::os::unix::fs::symlink(&planted.sessions, project.join("alias"))?;
    let name = (planted.author.strip_prefix(&planted.sessions)?).display();
    let inside = project.join(".sessions");
    std::fs::create_dir_all(inside.join("--fam--"))?;
    std::fs::write(
        inside.join("--fam--/100_author.jsonl"),
        "WALLED TRANSCRIPT\n",
    )?;
    let tree = yi_tools::Checkpoints::open(&home.join(".yi/checkpoints"), &project)?.capture()?;
    let show = yi_runtime::fetch::open_checkpoint_show(&home, &project)?;
    // The stores as the wiring hands them over: the broker's, the `--session-dir` among them.
    let stores = vec![planted.sessions.clone(), inside];
    let resolver = |workspace: &std::path::Path, wall: Wall| {
        yi_runtime::fetch::Resolver::new(workspace.to_path_buf(), wall)
            .with_member_trees(Arc::new(crate::permission_scope::MainTree(project.clone())))
            .with_checkpoint_show(Arc::clone(&show))
            .with_session_stores(stores.clone())
    };
    let fetched =
        |resolver: &yi_runtime::fetch::Resolver, url: &str| -> Result<String, Box<dyn Error>> {
            Ok(match resolver.fetch(&url.parse()?) {
                Ok(fetched) => fetched.text,
                Err(error) => error.to_string(),
            })
        };
    let tree = tree.as_str();
    let urls = [
        format!("tree://main/alias/{name}"),
        format!("local://alias/{name}"),
        format!("checkpoint://{tree}/alias/{name}"),
        "local://.sessions/--fam--/100_author.jsonl".to_owned(),
        format!("checkpoint://{tree}/.sessions/--fam--/100_author.jsonl"),
    ];
    let (walled, open) = (
        resolver(&project, juror_wall(&project)),
        resolver(&project, Wall::default()),
    );
    let at_home = resolver(&home, juror_wall(&project));
    let in_home = format!("local://.yi/sessions/{name}");
    for (resolver, url) in (urls.iter())
        .map(|url| (&walled, url))
        .chain([(&at_home, &in_home)])
    {
        let text = fetched(resolver, url)?;
        assert!(
            !text.contains("WALLED TRANSCRIPT"),
            "{url} reached a walled reader: {text}"
        );
    }
    for url in [&urls[0], &urls[3], &urls[4]] {
        let text = fetched(&open, url)?;
        assert!(
            text.contains("WALLED TRANSCRIPT"),
            "an unwalled {url}: {text}"
        );
    }
    Ok(())
}

/// A child wired the way `yi` wires one, walled by `wall`, its broker its parent's `for_child`
/// in `mode` with no sandbox, so every bash call it allows runs outside one.
fn wired_child(
    root: &std::path::Path,
    project: &std::path::Path,
    wall: Wall,
    mode: yi_permission::PermissionMode,
    store: Option<&std::path::Path>,
) -> (AgentSession, Arc<ProviderStream>) {
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    let mut broker = yi_runtime::PermissionBroker::new(
        mode,
        project.to_path_buf(),
        Vec::new(),
        None,
        session.events_sender(),
    );
    if let Some(store) = store {
        broker = broker.with_session_store(store);
    }
    let broker = Arc::new(broker.for_child(&wall, project));
    let _host = yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider: Arc::clone(&provider) as _,
            system_prompt: String::new(),
            tool_execution: ExecutionMode::Sequential,
            cwd: project.to_path_buf(),
            home: root.join("home"),
            lane_slots: 1,
            broker: Some(broker),
            tools: Arc::new(yi_tools::builtin_tools),
            depth: 1,
            max_depth: 2,
            rlm_dir: root.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall,
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: None,
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    (session, provider)
}

/// #979 review: the `--session-dir` reaches a walled reader's fetch through the real wiring
/// (`wire_fetch`), so `local://` into a store inside the workspace is refused there too.
#[tokio::test]
async fn a_wired_walled_fetch_reaches_no_transcript_in_the_session_dir() -> TestResult {
    let root = Scratch::new("yi-wall-wired-fetch")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let project = root.join("project");
    let store = project.join(".sessions");
    std::fs::create_dir_all(store.join("--fam--"))?;
    std::fs::write(store.join("--fam--/1_author.jsonl"), "WALLED TRANSCRIPT\n")?;
    let auto = yi_permission::PermissionMode::Auto;
    let (session, provider) =
        wired_child(&root, &project, juror_wall(&project), auto, Some(&store));
    let url = "local://.sessions/--fam--/1_author.jsonl";
    let seen = run_turns(&session, &provider, vec![("read", args(&[("path", url)]))]).await?;
    let (text, _) = seen.first().ok_or("no read")?;
    assert!(
        !text.contains("WALLED TRANSCRIPT"),
        "{url} reached a wired walled reader: {text}"
    );
    Ok(())
}

/// Another session's spill under a fake HOME, and a store holding the author's transcript and
/// the walled session's own, whose spill dir holds one file; returns the store and own paths.
fn plant_own(
    home: &std::path::Path,
    project: &std::path::Path,
) -> Result<(Planted, yi_session::SharedSession, [std::path::PathBuf; 2]), Box<dyn Error>> {
    let planted = plant_transcripts(home, project)?;
    let store = yi_session::create_flat_session(
        planted.own_dir.clone(),
        project.to_string_lossy(),
        Some("author".to_owned()),
    )?;
    let (id, transcript) = {
        let store = yi_session::lock_session(&store);
        (store.metadata().id.clone(), store.file_path().cloned())
    };
    let spills = home.join(".yi/spills");
    let own_spill = spills.join(id).join("0001.txt");
    for (path, text) in [
        (spills.join("author/0123.txt"), "WALLED OUTPUT\n"),
        (own_spill.clone(), "OWN SPILL\n"),
    ] {
        std::fs::create_dir_all(path.parent().ok_or("no parent")?)?;
        std::fs::write(path, text)?;
    }
    let transcript = transcript.ok_or("no transcript file")?;
    Ok((planted, store, [own_spill, transcript]))
}

/// #1001: a walled session's bash call that leaves the sandbox (yolo here, as an approval, a
/// rule or Linux sends one) met the wall only as command text, which named no spill root or
/// store; it now reads its own spill and transcript and no other session's, by any spelling.
#[tokio::test]
async fn a_walled_call_outside_the_sandbox_reads_no_other_sessions_store() -> TestResult {
    let root = Scratch::new("yi-wall-outside")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let project = root.join("project");
    let (planted, store, [own_spill, transcript]) = plant_own(&home, &project)?;
    let other = home.join(".yi/spills/author/0123.txt");
    let deep = project.join("a/b");
    std::fs::create_dir_all(&deep)?;
    let yolo = yi_permission::PermissionMode::Yolo;
    let (session, provider) = wired_child(&root, &project, juror_wall(&project), yolo, None);
    session.attach_store(store)?;
    let commands = [
        format!("cat {}", planted.author.display()),
        "cat ~/.yi/spil\\ls/author/0123.txt".to_owned(),
        "cat \"$HOME\"/.yi/*/author/0123.txt".to_owned(),
        format!(
            "cd {} && cat spills/author/0123.txt",
            home.join(".yi").display()
        ),
        format!("curl -s file://{}", other.display()),
        format!("curl -s -d @{} file:///dev/null", other.display()),
        // A `cd` the check follows, and a path glued to a flag.
        format!(
            "cd {} && cat ../../../home/.yi/spills/author/0123.txt",
            deep.display()
        ),
        format!(
            "tar -C{} -cf - author | tar -xOf -",
            home.join(".yi/spills").display()
        ),
        format!("cat {}", own_spill.display()),
        format!("cat {}", transcript.display()),
    ];
    let calls = (commands.iter())
        .map(|command| ("bash", args(&[("command", command)])))
        .collect();
    let seen = run_turns(&session, &provider, calls).await?;
    let [.., spill, own] = seen.as_slice() else {
        return Err(format!("ten calls, got {seen:?}").into());
    };
    for (command, (text, _)) in commands.iter().zip(&seen[..8]) {
        assert!(
            !text.contains("WALLED") && text.contains("runs outside the sandbox"),
            "{command} reached a walled session: {text}"
        );
    }
    assert!(spill.0.contains("OWN SPILL"), "own spill: {}", spill.0);
    let id = (own_spill.parent().and_then(std::path::Path::file_name)).ok_or("no id")?;
    let header = format!("\"id\":\"{}\"", id.to_string_lossy());
    assert!(own.0.contains(&header), "own transcript: {}", own.0);
    let open = wired_child(&root, &project, Wall::default(), yolo, None);
    let calls = vec![("bash", args(&[("command", &commands[0])]))];
    let (text, _) = run_turns(&open.0, &open.1, calls)
        .await?
        .pop()
        .ok_or("no call")?;
    assert!(
        text.contains("WALLED TRANSCRIPT"),
        "an unwalled call: {text}"
    );
    Ok(())
}

/// #1001 review: each relative `cd` doubled the dirs the host check resolves a word from, so a
/// long chain cost 2^k resolves per word; six distinct cds are the 64-dir cap, a seventh refuses.
#[tokio::test]
async fn a_cd_chain_past_the_host_checks_cap_is_refused_by_name() -> TestResult {
    let root = Scratch::new("yi-wall-cd-cap")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let project = root.join("project");
    let (_, store, _) = plant_own(&home, &project)?;
    let yolo = yi_permission::PermissionMode::Yolo;
    let (session, provider) = wired_child(&root, &project, juror_wall(&project), yolo, None);
    session.attach_store(store)?;
    let chain = |n: usize| {
        let cds: Vec<String> = ('a'..='z').take(n).map(|d| format!("cd {d}")).collect();
        format!("{}; echo CHAIN RAN", cds.join("; "))
    };
    let calls = [chain(6), chain(7)]
        .iter()
        .map(|command| ("bash", args(&[("command", command)])))
        .collect();
    let seen = run_turns(&session, &provider, calls).await?;
    let [at, past] = seen.as_slice() else {
        return Err(format!("two calls, got {seen:?}").into());
    };
    assert!(
        at.0.contains("CHAIN RAN"),
        "64 dirs are under the cap: {}",
        at.0
    );
    assert!(
        past.0.contains("128 directories")
            && past.0.contains("cap of 64")
            && !past.0.contains("CHAIN RAN"),
        "a seventh cd is refused by name: {}",
        past.0
    );
    Ok(())
}

/// #1001: a heartbeat's `exec://` source runs `sh -c` on the host on its cadence, and a walled
/// session's met only the permission broker: neither the wall's command-text check nor its roots.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_walled_heartbeat_source_names_no_walled_path() -> TestResult {
    let root = Scratch::new("yi-wall-heartbeat")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let project = root.join("project");
    let (planted, store, [own_spill, transcript]) = plant_own(&home, &project)?;
    std::fs::create_dir_all(project.join("secret"))?;
    std::fs::write(project.join("secret/key.txt"), "WALLED KEY\n")?;
    let mut wall = juror_wall(&project);
    wall.deny_read.push(project.join("secret"));
    let yolo = yi_permission::PermissionMode::Yolo;
    let (session, _) = wired_child(&root, &project, wall, yolo, None);
    session.attach_store(store)?;
    let heartbeats = session.heartbeat_service().ok_or("no heartbeats")?;
    heartbeats.bind_session("juror".to_owned());
    let mut host = yi_runtime::HostRegistry::default();
    heartbeats.register(&mut host);
    // Its own spill is spared, as its tools' is.
    for (path, walled) in [
        (project.join("secret/key.txt"), true),
        (planted.author, true),
        (home.join(".yi/spills/author/0123.txt"), true),
        (own_spill, false),
        (transcript, false),
    ] {
        let address = format!("exec://cat {}?every=30s", path.display());
        let payload = serde_json::json!({"address": address, "prompt": "watch"});
        let payload = payload.as_object().cloned().unwrap_or_default();
        let create =
            yi_kernel::client::HostHandlers::dispatch(&host, "rlm_heartbeat.create", payload);
        let made = create
            .ok_or("rlm_heartbeat.create is not registered")?
            .await;
        let refused = made
            .as_ref()
            .is_err_and(|text| text.contains("reviewer wall"));
        assert_eq!(refused, walled, "{address}: {made:?}");
    }
    Ok(())
}

/// #1003: a user's gate rule that names a command stops a heartbeat's `exec://` source as it
/// stops the same command armed from a todo; a command the rule does not name still arms.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_user_rule_stops_a_heartbeat_source_it_names() -> TestResult {
    let root = Scratch::new("yi-rule-heartbeat")?;
    let project = root.join("project");
    std::fs::create_dir_all(root.join("home/.yi/rules"))?;
    std::fs::create_dir_all(&project)?;
    std::fs::write(
        root.join("home/.yi/rules/no-denied.md"),
        "---\ntrigger: denied-marker\nscope: tool:bash\nmode: gate\n---\nNever run denied-marker.\n",
    )?;
    let yolo = yi_permission::PermissionMode::Yolo;
    let (session, _) = wired_child(&root, &project, Wall::default(), yolo, None);
    let heartbeats = session.heartbeat_service().ok_or("no heartbeats")?;
    heartbeats.bind_session("juror".to_owned());
    let mut host = yi_runtime::HostRegistry::default();
    heartbeats.register(&mut host);
    for (command, denied) in [
        (format!("touch {}/denied-marker", project.display()), true),
        (format!("touch {}/other", project.display()), false),
    ] {
        let payload = serde_json::json!({"address": format!("exec://{command}?every=30s"), "prompt": "watch"});
        let payload = payload.as_object().cloned().unwrap_or_default();
        let made =
            yi_kernel::client::HostHandlers::dispatch(&host, "rlm_heartbeat.create", payload)
                .ok_or("rlm_heartbeat.create is not registered")?
                .await;
        let refused = made
            .as_ref()
            .is_err_and(|text| text.contains("Denied by rule"));
        assert_eq!(refused, denied, "{command}: {made:?}");
        assert!(!project.join("denied-marker").exists());
    }
    Ok(())
}

/// #1001: with no Seatbelt (Linux) a walled session's kernel and its `bash()` jobs would run with
/// no profile, so a cell would meet no wall at all; it never boots, and the cell says why.
#[tokio::test]
async fn a_walled_kernel_with_no_sandbox_never_boots() -> TestResult {
    if yi_tools::Sandbox::available() {
        return Ok(());
    }
    let root = Scratch::new("yi-wall-kernel")?;
    let home = root.home()?;
    // SAFETY: nextest runs each test in its own process; no other test reads HOME.
    unsafe { std::env::set_var("HOME", &home) };
    let project = root.join("project");
    std::fs::create_dir_all(&project)?;
    let auto = yi_permission::PermissionMode::Auto;
    let (session, provider) = wired_child(&root, &project, juror_wall(&project), auto, None);
    let cell = vec![("ipython", args(&[("code", "print('cell ' + 'ran')")]))];
    let seen = run_turns(&session, &provider, cell).await?;
    let (text, is_error) = seen.first().ok_or("no cell")?;
    assert!(
        *is_error && text.contains("no Seatbelt") && !text.contains("cell ran"),
        "{text}"
    );
    Ok(())
}
