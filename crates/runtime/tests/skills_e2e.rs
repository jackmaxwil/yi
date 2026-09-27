#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;
use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Value, json};
use yi_loop::ExecutionMode;
use yi_runtime::{
    AgentSession, HostRegistry, KernelService, KernelServiceOptions, ProviderStream, SessionConfig,
};
use yi_tools::{CancelFlag, KernelBridge, ToolContext};
use yi_types::message::Content;
use yi_types::model::{Model, ModelCost};

type TestResult = Result<(), Box<dyn Error>>;

fn vision_faux_model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned(), "image".to_owned()],
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

async fn cell(
    service: &Arc<KernelService>,
    code: impl Into<String>,
) -> Result<yi_tools::KernelCellOutcome, String> {
    let service = Arc::clone(service);
    let code = code.into();
    tokio::task::spawn_blocking(move || {
        let cancelled: CancelFlag = Arc::new(|| false);
        KernelBridge::execute_cell(service.as_ref(), &code, &cancelled)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tokio::test]
async fn bundled_python_skills_work_through_the_kernel() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None, None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: vision_faux_model(),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    session.enable_compaction();
    let compactor = session.compactor().ok_or("no compactor")?;

    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    {
        let compactor = Arc::clone(&compactor);
        registry.register("compact.run", move |payload| {
            let instructions = payload
                .get("instructions")
                .and_then(Value::as_str)
                .map(str::to_owned);
            compactor.schedule_with_instructions(instructions);
            Box::pin(async {
                let mut reply = serde_json::Map::new();
                reply.insert("scheduled".to_owned(), Value::Bool(true));
                Ok(reply)
            })
        });
    }
    {
        let status = session.compact_status_handle().ok_or("no status handle")?;
        registry.register("compact.status", move |_payload| {
            let status = status();
            Box::pin(async move {
                let mut reply = serde_json::Map::new();
                reply.insert("tokens".to_owned(), Value::from(status.tokens));
                reply.insert(
                    "context_window".to_owned(),
                    Value::from(status.context_window),
                );
                reply.insert("percent".to_owned(), Value::from(status.percent));
                reply.insert("scheduled".to_owned(), Value::Bool(status.scheduled));
                Ok(reply)
            })
        });
    }
    let model = session.model();
    registry.register("model.info", move |_payload| {
        let reply = json!({
            "provider": model.provider,
            "id": model.id,
            "name": model.name,
            "selector": format!("{}/{}", model.provider, model.id),
            "input": model.input,
        })
        .as_object()
        .cloned()
        .unwrap_or_default();
        Box::pin(async move { Ok(reply) })
    });
    yi_runtime::wiring::register_history_grep(&mut registry, session.store_handle());

    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    }));

    let status_cell = cell(
        &service,
        "s = await compact.status()\nprint(s['context_window'], s['scheduled'])",
    )
    .await
    .map_err(|error| error.to_string())?;
    assert!(
        status_cell.result.stdout.contains("128000 False"),
        "compact.status must surface real usage: {} {}",
        status_cell.result.stdout,
        status_cell.result.stderr
    );

    let run_cell = cell(
        &service,
        "r = await compact.run('keep the failing test names')\nprint(r)",
    )
    .await
    .map_err(|error| error.to_string())?;
    assert!(
        run_cell.result.stdout.contains("'scheduled': True"),
        "compact.run must schedule: {} {}",
        run_cell.result.stdout,
        run_cell.result.stderr
    );
    assert!(
        compactor.scheduled(),
        "a kernel compact.run must set the host compactor pending"
    );

    // Recall round-trip: compact.recall greps the attached store by needle.
    let store = Arc::new(std::sync::Mutex::new(yi_session::SessionStore::in_memory(
        yi_session::SessionMetadata {
            id: "skills-e2e".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        },
    )));
    let needle_id = yi_session::lock_session(&store).append_message(
        "main",
        yi_types::message::AgentMessage::user_input(
            yi_types::message::UserContent::Text("the ZEBRA-7712 needle turn".to_owned()),
            0,
        ),
    )?;
    session.attach_store(store)?;
    let recall_cell = cell(
        &service,
        "h = await compact.recall('ZEBRA-7712')\nprint(h['hits'][0]['entryId'], h['hits'][0]['type'])",
    )
    .await
    .map_err(|error| error.to_string())?;
    assert!(
        recall_cell
            .result
            .stdout
            .contains(&format!("{needle_id} message")),
        "compact.recall must return the needle entry id: {} {}",
        recall_cell.result.stdout,
        recall_cell.result.stderr
    );

    // The call a refused read names runs as named, on an image under a name with a quote and a
    // backslash that a bare "{path}" literal breaks on (the 2026-09-11 v4 sweep's image reads).
    let dir = Scratch::new("yi-skills-attach")?;
    let image = dir.join("sch\"em\\atic.png");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/documents/checker.png"
        ),
        &image,
    )?;
    let read = yi_tools::builtin_tools()
        .into_iter()
        .find(|tool| tool.name() == "read")
        .ok_or("no read tool")?;
    let refused = read.execute(
        serde_json::from_value(json!({"path": image}))?,
        &ToolContext::new(dir.to_path_buf()),
    );
    let refusal: String = refused
        .result
        .content
        .iter()
        .map(|content| match content {
            Content::Text { text, .. } => text.as_str(),
            _ => "",
        })
        .collect();
    let named = refusal.split('`').nth(1).ok_or_else(|| refusal.clone())?;
    let attach_cell = cell(&service, named).await?;
    assert!(
        attach_cell
            .result
            .stdout
            .contains("Loaded 1 image(s) into context"),
        "{named} must confirm the load: {} {:?}",
        attach_cell.result.stdout,
        attach_cell.result.error
    );
    assert_eq!(
        attach_cell.result.attachments.len(),
        1,
        "the display_data attachment must reach the host reducer"
    );
    assert_eq!(attach_cell.result.attachments[0].mime_type, "image/png");

    service.dispose().await;
    Ok(())
}

fn skill_dir(root: &std::path::Path, name: &str, frontmatter: &str) -> TestResult {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("SKILL.md"), frontmatter)?;
    Ok(())
}

#[test]
fn the_catalog_lists_both_roots_and_the_project_shadows_the_global() -> TestResult {
    let root = Scratch::new("yi-skills-catalog")?;
    let home = root.join("home");
    let project = root.join("project");
    std::fs::create_dir_all(home.join(".yi/skills"))?;
    std::fs::create_dir_all(project.join(".yi/skills"))?;
    skill_dir(
        &home.join(".yi/skills"),
        "brainstorm",
        "---\nname: brainstorm\ndescription: \"Use before creative work.\"\n---\nbody\n",
    )?;
    skill_dir(
        &home.join(".yi/skills"),
        "shared",
        "---\nname: shared\ndescription: The global copy.\n---\nbody\n",
    )?;
    skill_dir(
        &project.join(".yi/skills"),
        "shared",
        "---\nname: shared\ndescription: The project copy.\n---\nbody\n",
    )?;

    let catalog = yi_runtime::skills_catalog(&project, &home, yi_runtime::Bytes(16_384))
        .ok_or("no catalog")?;
    assert!(!catalog.truncated);
    assert!(
        catalog
            .text
            .contains("brainstorm: Use before creative work."),
        "{}",
        catalog.text
    );
    assert!(
        catalog.text.contains("shared: The project copy."),
        "{}",
        catalog.text
    );
    assert!(
        !catalog.text.contains("The global copy."),
        "{}",
        catalog.text
    );
    assert!(
        catalog.text.contains(
            &project
                .join(".yi/skills/shared/SKILL.md")
                .display()
                .to_string()
        ),
        "the catalog must locate the file: {}",
        catalog.text
    );

    let tight =
        yi_runtime::skills_catalog(&project, &home, yi_runtime::Bytes(80)).ok_or("no catalog")?;
    assert!(tight.truncated, "a catalog over budget must say so");
    assert!(tight.text.len() < catalog.text.len());
    Ok(())
}

#[test]
fn no_skills_roots_means_no_catalog_block() -> TestResult {
    let root = Scratch::new("yi-skills-empty")?;
    assert!(yi_runtime::skills_catalog(&root, &root, yi_runtime::Bytes(16_384)).is_none());
    Ok(())
}

#[test]
fn a_bundle_layout_is_walked_one_level_deeper() -> TestResult {
    let root = Scratch::new("yi-skills-bundle")?;
    let global = root.join("home/.yi/skills");
    std::fs::create_dir_all(global.join("caveman"))?;
    skill_dir(
        &global.join("caveman"),
        "surgical-patch",
        "---\nname: surgical-patch\ndescription: Small bounded edits.\n---\nbody\n",
    )?;
    let catalog = yi_runtime::skills_catalog(&root, &root.join("home"), yi_runtime::Bytes(16_384))
        .ok_or("no catalog")?;
    assert!(
        catalog
            .text
            .contains("surgical-patch: Small bounded edits."),
        "{}",
        catalog.text
    );
    Ok(())
}

#[test]
fn a_folded_description_reads_as_its_sentence() -> TestResult {
    let root = Scratch::new("yi-skills-folded")?;
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".yi/skills"))?;
    skill_dir(
        &home.join(".yi/skills"),
        "review",
        "---\nname: review\ndescription: >\n  Verify finished work against its acceptance criteria with a cold-context\n  reviewer. Use before declaring a goal or large task complete: not a\n  code-style review.\n---\n\n# Review\n",
    )?;
    skill_dir(
        &home.join(".yi/skills"),
        "literal",
        "---\nname: literal\ndescription: |\n  first line\n  second line\n---\nbody\n",
    )?;
    let catalog =
        yi_runtime::skills_catalog(&home, &home, yi_runtime::Bytes(16_384)).ok_or("no catalog")?;
    assert!(
        catalog.text.contains(
            "review: Verify finished work against its acceptance criteria with a cold-context reviewer. Use before declaring a goal or large task complete: not a code-style review."
        ),
        "{}",
        catalog.text
    );
    assert!(
        catalog.text.contains("literal: first line\nsecond line"),
        "{}",
        catalog.text
    );
    assert!(!catalog.text.contains("review: >"), "{}", catalog.text);
    Ok(())
}

#[test]
fn a_project_under_home_keeps_its_skills_project_scoped() -> TestResult {
    let home = Scratch::new("yi-skills-under-home")?;
    let project = home.join("Development/project");
    std::fs::create_dir_all(home.join(".yi/skills"))?;
    std::fs::create_dir_all(project.join(".agents/skills"))?;
    skill_dir(
        &home.join(".yi/skills"),
        "global-one",
        "---\nname: global-one\ndescription: Global.\n---\nbody\n",
    )?;
    skill_dir(
        &project.join(".agents/skills"),
        "repo-one",
        "---\nname: repo-one\ndescription: Repository.\n---\nbody\n",
    )?;
    let (global, project_skills) = yi_runtime::skills::discover_split(&project, &home);
    assert_eq!(
        global
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["global-one"]
    );
    assert_eq!(
        project_skills
            .iter()
            .map(|skill| skill.name.as_str())
            .collect::<Vec<_>>(),
        vec!["repo-one"]
    );
    Ok(())
}

#[test]
fn the_catalog_ladder_keeps_every_name_and_clips_descriptions_first() -> TestResult {
    let root = Scratch::new("yi-skills-ladder")?;
    let home = root.join("home");
    std::fs::create_dir_all(home.join(".yi/skills"))?;
    let long = "x".repeat(300);
    for index in 0..40 {
        skill_dir(
            &home.join(".yi/skills"),
            &format!("skill-{index:02}"),
            &format!("---\nname: skill-{index:02}\ndescription: {long}\n---\nbody\n"),
        )?;
    }
    let full =
        yi_runtime::skills_catalog(&home, &home, yi_runtime::Bytes(1 << 20)).ok_or("no catalog")?;
    assert!(!full.truncated);
    assert_eq!(full.text.matches(&long).count(), 40);

    let clipped =
        yi_runtime::skills_catalog(&home, &home, yi_runtime::Bytes(8_192)).ok_or("no catalog")?;
    assert!(
        !clipped.text.contains(&long),
        "descriptions clip before names go"
    );
    for index in 0..40 {
        assert!(
            clipped.text.contains(&format!("skill-{index:02}")),
            "every name survives the budget: {}",
            clipped.text
        );
    }
    assert!(clipped.text.len() <= 8_192);
    assert!(!clipped.text.contains("[... truncated"), "{}", clipped.text);

    let tight =
        yi_runtime::skills_catalog(&home, &home, yi_runtime::Bytes(2_048)).ok_or("no catalog")?;
    assert!(tight.truncated);
    assert!(tight.text.contains("more: "), "{}", tight.text);
    assert!(
        tight.text.contains("skill-39"),
        "the last name rides the more line: {}",
        tight.text
    );
    assert_eq!(
        yi_runtime::skills::catalog_budget(128_000),
        yi_runtime::Bytes(10_240)
    );
    assert_eq!(
        yi_runtime::skills::catalog_budget(1_300_000),
        yi_runtime::skills::CATALOG_CEILING
    );
    assert_eq!(
        yi_runtime::skills::catalog_budget(8_000),
        yi_runtime::skills::CATALOG_FLOOR
    );
    Ok(())
}

#[tokio::test]
async fn save_read_forget_through_the_kernel() -> TestResult {
    let base = Scratch::new("yi-skills-memory")?;
    std::fs::create_dir_all(base.join("cwd"))?;
    let mut registry = HostRegistry::default();
    registry.register_mcp_stubs();
    yi_runtime::memory::attach(
        None,
        &mut registry,
        base.join("home"),
        base.join("cwd"),
        true,
    );
    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: base.join("cwd"),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    }));
    let out = cell(
        &service,
        "r = await memory.save(\"\"\"---\nname: harbor-json-stderr\ndescription: harbor exec merges stderr; parse --json from the first [ line\ntype: project\nscpe: repo\n---\nA --json command under harbor arrives with its stderr first.\n\"\"\")\nprint(r['name'], r['scope'], r['updated'], r['warnings'])\nn = await memory.read('harbor exec merges stderr; parse --json from the first [ line')\nprint(n['text'])\nprint((await memory.forget('harbor-json-stderr'))['name'])\ntry:\n    await memory.read('harbor-json-stderr')\nexcept Exception as error:\n    print('gone:', error)",
    )
    .await?;
    let stdout = out.result.stdout;
    assert!(
        stdout
            .contains("harbor-json-stderr repo False ['ignored key `scpe`; did you mean `scope`']"),
        "{stdout} {}",
        out.result.stderr
    );
    assert!(stdout.contains("A --json command under harbor arrives with its stderr first."));
    assert!(
        stdout.contains("gone:") && stdout.contains("no note matches"),
        "{stdout}"
    );
    assert!(!stdout.contains(".yi"), "a reply leaked a path: {stdout}");
    service.dispose().await;
    Ok(())
}
