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
    Ok(())
}
