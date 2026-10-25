use std::error::Error;

use yi_runtime::{builtin_tools, doctrine_fragment, identity_fragment};

type TestResult = Result<(), Box<dyn Error>>;

/// identity.md once described grep as literal-only while the tool had been regex
/// for weeks: a prompt claim about a tool is checked against the tool, not trusted.
#[test]
fn identity_names_every_tool_the_session_registers() -> TestResult {
    let identity = identity_fragment();
    for tool in builtin_tools() {
        assert!(
            identity.contains(tool.name()),
            "identity.md does not name the {} tool",
            tool.name()
        );
    }
    for name in [
        yi_runtime::todo::tool::NAME,
        "plan",
        "get_context",
        "ipython",
    ] {
        assert!(identity.contains(name), "identity.md does not name {name}");
    }
    Ok(())
}

/// A leaf root child keeps the kernel-or-bash teaching and Planning's conduct and loses the
/// delegation rules and the lift rule; the root reads all four.
#[test]
fn a_leaf_is_taught_no_delegation_and_the_root_all_of_it() -> TestResult {
    let dir = crate::scratch::Scratch::new("yi-leaf-doctrine")?;
    let prompt = |can_delegate: bool| {
        let mut host = yi_runtime::ext::install(yi_runtime::ExtOptions {
            cwd: dir.to_path_buf(),
            home: dir.to_path_buf(),
            mode: yi_runtime::PermissionMode::Auto,
            user_system: String::new(),
            schema_instruction: None,
            context_window: 128_000,
            global_skills: Vec::new(),
            can_delegate,
        });
        host.start(None, false);
        host.system_prompt()
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
    };
    let (root, leaf) = (prompt(true), prompt(false));
    for kept in ["Bash or the kernel", "Ground unknowns by exploring"] {
        assert!(root.contains(kept), "the root lost {kept}");
        assert!(leaf.contains(kept), "the leaf lost {kept}");
    }
    for taught in ["Solo, readers, or writers", "Lift todos into the plan tool"] {
        assert!(root.contains(taught), "the root lost {taught}");
        assert!(!leaf.contains(taught), "the leaf is taught {taught}");
    }
    Ok(())
}

#[test]
fn every_flag_the_prompt_names_for_grep_is_in_its_schema() -> TestResult {
    let grep = builtin_tools()
        .into_iter()
        .find(|tool| tool.name() == "grep")
        .ok_or("no grep tool")?;
    let schema = grep.schema();
    let properties = schema
        .get("properties")
        .and_then(|value| value.as_object())
        .ok_or("grep schema has no properties")?;
    for flag in ["literal", "multiline", "type"] {
        assert!(
            identity_fragment().contains(flag),
            "identity.md no longer mentions grep's {flag}"
        );
        assert!(properties.contains_key(flag), "grep schema lost {flag}");
    }
    Ok(())
}

#[test]
fn doctrine_names_only_ops_the_todo_tool_accepts() -> TestResult {
    let schema = yi_runtime::todo::tool::schema();
    let ops: Vec<String> = schema["properties"]["op"]["enum"]
        .as_array()
        .ok_or("no op enum")?
        .iter()
        .filter_map(|value| value.as_str().map(str::to_owned))
        .collect();
    for op in [
        "start", "done", "block", "unblock", "drop", "append", "view",
    ] {
        assert!(ops.contains(&op.to_owned()), "the todo tool lost `{op}`");
        assert!(
            doctrine_fragment().contains(&format!("`{op}")),
            "doctrine no longer teaches `{op}`"
        );
    }
    Ok(())
}

/// Dies with the text a model reads behind the engine: with a plan open the todo tool sold its
/// ops as the list's, and the plan schema offered a worktree with no contract (D223, D226).
#[test]
fn the_tool_text_says_the_plan_is_the_list_and_a_worktree_needs_a_contract() -> TestResult {
    let todo = yi_runtime::todo::tool::DESCRIPTION;
    assert!(
        todo.contains("While a plan is open the plan is the list"),
        "{todo}"
    );
    let schema = yi_runtime::plan::tool::schema();
    let todos = schema["properties"]["todos"]["description"]
        .as_str()
        .ok_or("no todos text")?;
    assert!(
        todos.contains("isolation worktree requires a contract"),
        "{todos}"
    );
    assert!(!todos.contains("completes unverified"), "{todos}");
    Ok(())
}

/// Listing formats in `read`'s description is a claim about the installed wheel, so the list is
/// checked against the wheel itself, both ways.
#[test]
fn read_names_exactly_the_formats_the_installed_wheel_converts() -> TestResult {
    let home = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("HOME is unset")?);
    let python =
        yi_kernel::bootstrap::ensure_kernel_python(&yi_kernel::bootstrap::BootstrapOptions {
            on_progress: None,
            home: home.clone(),
            runtime_source_dir: yi_kernel::bootstrap::default_runtime_source_dir(),
            skills_source_dir: yi_kernel::bootstrap::default_skills_source_dir(),
            toolchain: None,
            venv_dir: None,
        })?;
    let probe = yi_tools::command(&python)
        .args([
            "-c",
            "import json, typing, anydoc; print(json.dumps([kind for kind in typing.get_args(anydoc.Format) if kind != 'csv']))",
        ])
        .output()?;
    let live: Vec<String> = serde_json::from_slice(&probe.stdout)?;
    let read = yi_runtime::builtin_tools_with(false, Some(yi_runtime::documents(&home)))
        .into_iter()
        .find(|tool| tool.name() == "read")
        .ok_or("no read tool")?;
    let claimed: Vec<String> = read
        .description()
        .split("converted to Markdown: ")
        .nth(1)
        .and_then(|rest| rest.split('.').next())
        .map(|list| {
            let mut depth = 0_u32;
            let canonical: String = list
                .chars()
                .filter(|character| {
                    depth = match character {
                        '(' => depth.saturating_add(1),
                        ')' => depth.saturating_sub(1),
                        _ => depth,
                    };
                    depth == 0 && *character != ')'
                })
                .collect();
            canonical
                .split(", ")
                .map(|entry| entry.trim().to_owned())
                .collect()
        })
        .unwrap_or_default();
    assert!(!live.is_empty(), "the venv carries the converter");
    assert_eq!(claimed, live);
    Ok(())
}

/// The field list a schema block renders: one line per property, in the schema's order, with its
/// type, whether it is required, and its description.
fn render_fields(schema: &serde_json::Value) -> String {
    let required: Vec<&str> = schema["required"]
        .as_array()
        .map(|names| names.iter().filter_map(|name| name.as_str()).collect())
        .unwrap_or_default();
    let kind = |field: &serde_json::Value| match field["type"].as_str() {
        Some("array") => format!(
            "list of {}",
            field["items"]["type"].as_str().unwrap_or("value")
        ),
        Some(name) => name.to_owned(),
        None => "value".to_owned(),
    };
    let mut out = Vec::new();
    for (name, field) in schema["properties"].as_object().into_iter().flatten() {
        let mut facts = vec![kind(field)];
        if required.contains(&name.as_str()) {
            facts.push("required".to_owned());
        }
        if let Some(values) = field["enum"].as_array() {
            let values: Vec<String> = values
                .iter()
                .map(|v| format!("`{}`", v.as_str().unwrap_or("")))
                .collect();
            facts.push(format!("one of {}", values.join(", ")));
        }
        let description = field["description"].as_str().unwrap_or("");
        let line = format!("- `{name}` ({})", facts.join(", "));
        out.push(if description.is_empty() {
            line
        } else {
            format!("{line}: {description}")
        });
    }
    out.join("\n")
}

const BLOCK_OPEN: &str = "<!-- yi:schema ";
const BLOCK_CLOSE: &str = "<!-- /yi:schema -->";

/// Every schema block in `text`, re-rendered from the live schema it names.
fn rendered(text: &str) -> Result<(String, Vec<String>), Box<dyn Error>> {
    let mut out = String::new();
    let mut blocks = Vec::new();
    let mut rest = text;
    while let Some(at) = rest.find(BLOCK_OPEN) {
        let (before, from) = rest.split_at(at);
        out.push_str(before);
        let header_end = from.find("-->").ok_or("an unclosed block header")?;
        let header = from
            .get(BLOCK_OPEN.len()..header_end)
            .ok_or("header")?
            .trim();
        let (tool, pointer) = header
            .split_once(' ')
            .ok_or("a header names a tool and a pointer")?;
        let schema = match tool {
            "plan" => yi_runtime::plan::tool::schema(),
            "todo" => yi_runtime::todo::tool::schema(),
            other => return Err(format!("no schema named {other}").into()),
        };
        let at_pointer = schema
            .pointer(pointer)
            .ok_or(format!("{tool} has no {pointer}"))?;
        let close = from.find(BLOCK_CLOSE).ok_or("a block without its close")?;
        out.push_str(from.get(..header_end + 3).ok_or("header")?);
        out.push('\n');
        out.push_str(&render_fields(at_pointer));
        out.push('\n');
        out.push_str(BLOCK_CLOSE);
        blocks.push(format!("{tool} {pointer}"));
        rest = from.get(close + BLOCK_CLOSE.len()..).ok_or("tail")?;
    }
    out.push_str(rest);
    Ok((out, blocks))
}

/// Dies with a prompt or skill teaching fields its tool does not take: the plan skill still said
/// title, acceptance, check and deps after the orchestrate protocol was fixed by hand.
#[test]
fn every_schema_block_is_the_schemas_own_rendering() -> TestResult {
    let manifest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let root = manifest.ancestors().nth(2).ok_or("the repository root")?;
    let skills = root.join("skills");
    let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(manifest.join("src/prompts"))?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "md"))
        .collect();
    for set in std::fs::read_dir(&skills)?.flatten() {
        for skill in std::fs::read_dir(set.path())?.flatten() {
            let manifest = skill.path().join("SKILL.md");
            if manifest.is_file() {
                files.push(manifest);
            }
        }
    }
    let bless = std::env::var_os("YI_BLESS").is_some();
    let mut stale = Vec::new();
    let mut found = Vec::new();
    for path in files {
        let text = std::fs::read_to_string(&path)?;
        let (fresh, blocks) = rendered(&text)?;
        let name = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        found.extend(blocks.into_iter().map(|block| format!("{name}: {block}")));
        if fresh != text {
            if bless {
                std::fs::write(&path, fresh)?;
            } else {
                stale.push(name);
            }
        }
    }
    assert!(
        stale.is_empty(),
        "stale schema blocks in {stale:?}; run `just schema-blocks`"
    );
    for owed in [
        "crates/runtime/src/prompts/orchestrate.md: plan /properties/todos/items",
        "skills/yi/plan/SKILL.md: plan /properties/todos/items",
        "skills/yi/review/SKILL.md: plan /properties/todos/items/properties/contract/properties/items/items",
    ] {
        assert!(
            found.iter().any(|block| block == owed),
            "missing block {owed}; found {found:?}"
        );
    }
    Ok(())
}

/// The recorded session `01a13357`, written by an earlier build's storage code: its v1
/// `ext_state` carries every slot's text and a granted AGENTS.md of 43 KB in the yard.
const RECORDED: &str = include_str!("../../tui/tests/fixtures/sessions/01a13357.jsonl");

fn recorded_snapshot() -> Result<serde_json::Value, Box<dyn Error>> {
    for line in RECORDED.lines() {
        let entry: serde_json::Value = serde_json::from_str(line)?;
        if entry["customType"] == "ext_state" {
            return Ok(entry["data"].clone());
        }
    }
    Err("the recording lost its ext_state".into())
}

fn installed(dir: &std::path::Path, mode: yi_runtime::PermissionMode) -> yi_runtime::ext::Host {
    yi_runtime::ext::install(yi_runtime::ExtOptions {
        cwd: dir.to_path_buf(),
        home: dir.to_path_buf(),
        mode,
        user_system: String::new(),
        schema_instruction: None,
        context_window: 128_000,
        global_skills: Vec::new(),
        can_delegate: true,
    })
}

/// A project whose AGENTS.md is the recorded one, and a session store beside it.
fn project(
    name: &str,
) -> Result<(crate::scratch::Scratch, yi_session::SharedSession), Box<dyn Error>> {
    let dir = crate::scratch::Scratch::new(name)?;
    let agents = recorded_snapshot()?["yard"]
        .as_array()
        .and_then(|yard| yard.iter().find(|entry| entry["source"] == "AGENTS.md"))
        .and_then(|entry| entry["text"].as_str())
        .ok_or("no recorded AGENTS.md")?
        .to_owned();
    std::fs::write(dir.join("AGENTS.md"), agents)?;
    let mut repo = yi_session::JsonlRepo::new(dir.join("sessions"), dir.display().to_string());
    let store = {
        use yi_session::SessionRepo;
        repo.create(yi_session::CreateOptions::default())?
    };
    Ok((dir, store))
}

fn ext_states(store: &yi_session::SharedSession) -> Result<Vec<String>, Box<dyn Error>> {
    let file = yi_session::lock_session(store)
        .file_path()
        .cloned()
        .ok_or("no session file")?;
    Ok(std::fs::read_to_string(file)?
        .lines()
        .filter(|line| line.contains("\"customType\":\"ext_state\""))
        .map(str::to_owned)
        .collect())
}

/// Dies with the control: snapshot every slot's text and the session file carries the whole
/// doctrine. The cache case: a resume under the same build and mode sends the same bytes.
#[test]
fn a_resume_under_the_same_build_sends_the_same_prompt_and_stores_hashes() -> TestResult {
    let (dir, store) = project("yi-prompt-same")?;
    let mut first = installed(&dir, yi_runtime::PermissionMode::Auto);
    first.start(Some(&store), false);
    let trigger = first.prompt_event("refactor and migrate and split crates/a and crates/b");
    first.dispatch(&trigger, Some(&store));
    let sent = first.system_prompt();
    assert!(
        sent.contains("# Orchestrate"),
        "the trigger attached the protocol"
    );
    let mut resumed = installed(&dir, yi_runtime::PermissionMode::Auto);
    resumed.start(Some(&store), true);
    assert_eq!(
        resumed.system_prompt(),
        sent,
        "a resume rebuilds the same bytes"
    );
    let states = ext_states(&store)?;
    let doctrine = yi_runtime::doctrine_fragment()
        .lines()
        .next()
        .unwrap_or_default();
    assert!(
        states.iter().all(|line| !line.contains(doctrine)),
        "the doctrine is the build's: the file keeps its hash, never its text"
    );
    Ok(())
}

/// Dies with the control: restore the snapshot wholesale and the old mode's fragment
/// overwrites the one this run was started with.
#[test]
fn a_resume_under_another_mode_tells_the_model_the_new_mode() -> TestResult {
    let (dir, store) = project("yi-prompt-mode")?;
    let mut first = installed(&dir, yi_runtime::PermissionMode::Auto);
    first.start(Some(&store), false);
    let _frozen = first.system_prompt();
    let mut resumed = installed(&dir, yi_runtime::PermissionMode::Yolo);
    resumed.start(Some(&store), true);
    let prompt = resumed.system_prompt();
    let (yolo, auto) = (
        yi_permission::mode_fragment(yi_runtime::PermissionMode::Yolo),
        yi_permission::mode_fragment(yi_runtime::PermissionMode::Auto),
    );
    assert!(prompt.contains(yolo.trim_end()), "the model is told yolo");
    assert!(!prompt.contains(auto.trim_end()), "the old mode is gone");
    Ok(())
}

/// A v1 snapshot an earlier build recorded, stored and reopened by the session store, resumes:
/// build slots come from this build, extension slots and the yard with their recorded text.
#[test]
fn a_v1_snapshot_from_a_recorded_session_restores() -> TestResult {
    let dir = crate::scratch::Scratch::new("yi-prompt-v1")?;
    let mut repo = yi_session::JsonlRepo::new(dir.join("sessions"), dir.display().to_string());
    let recorded = recorded_snapshot()?;
    let file = {
        use yi_session::SessionRepo;
        let store = repo.create(yi_session::CreateOptions::default())?;
        let mut session = yi_session::lock_session(&store);
        session.append_custom("main", "ext_state", Some(recorded.clone()))?;
        session.file_path().cloned().ok_or("no session file")?
    };
    let store = yi_session::open_shared(&file).ok_or("the session file does not open")?;
    let text_of = |name: &str| {
        recorded["slots"]
            .as_array()
            .and_then(|slots| slots.iter().find(|slot| slot["name"] == name))
            .and_then(|slot| slot["text"].as_str())
            .map(str::to_owned)
    };
    let mut resumed = installed(&dir, yi_runtime::PermissionMode::Auto);
    resumed.start(Some(&store), true);
    let state = resumed.state();
    let doctrine = yi_runtime::ext::Slot::new(yi_runtime::ext::Rank::Doctrine, "doctrine");
    assert_eq!(
        state.slot_text(&doctrine),
        Some(yi_runtime::doctrine_fragment())
    );
    assert_ne!(
        text_of("doctrine").as_deref(),
        Some(yi_runtime::doctrine_fragment())
    );
    let ripwire = yi_runtime::ext::Slot::new(yi_runtime::ext::Rank::Tool, "ripwire");
    assert_eq!(state.slot_text(&ripwire), text_of("ripwire").as_deref());
    assert!(
        resumed.system_prompt().contains("trust=\"granted\""),
        "the recorded granted AGENTS.md rides the yard"
    );
    Ok(())
}

/// Dies with the control: a compaction read before the first prompt freezes the prompt
/// before the session start attaches the project's instructions.
#[tokio::test]
async fn compacting_before_the_first_prompt_keeps_the_project_instructions() -> TestResult {
    let (dir, _store) = project("yi-prompt-compact")?;
    let provider = std::sync::Arc::new(yi_runtime::ProviderStream::new(None));
    provider.queue_faux(vec![yi_ai::faux::faux_assistant_message(
        vec![yi_ai::faux::faux_text("hello")],
        yi_types::message::StopReason::Stop,
    )]);
    let mut session = yi_runtime::AgentSession::new(
        yi_runtime::SessionConfig {
            system_prompt: "sys".to_owned(),
            model: crate::support::faux_model(),
            thinking_level: None,
            tool_execution: yi_loop::ExecutionMode::Sequential,
        },
        provider,
    );
    session.enable_compaction_with(yi_context::Settings::default());
    session.install_extensions(installed(&dir, yi_runtime::PermissionMode::Auto));
    let _nothing_to_compact = session.compact_now().await;
    session.prompt("hi")?;
    session.wait_idle().await;
    assert!(
        session.system_prompt().contains("source=\"AGENTS.md\""),
        "the first request carries the project's instructions"
    );
    Ok(())
}
