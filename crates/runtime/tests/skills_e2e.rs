use crate::scratch;
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
    Model {
        id: "faux-1".to_owned(),
        name: "Faux".to_owned(),
        api: "faux".to_owned(),
        provider: "faux".to_owned(),
        base_url: "http://localhost:0".to_owned(),
        reasoning: false,
        input: vec!["text".to_owned(), "image".to_owned()],
        cost: ModelCost::free(),
        context_window: 128_000,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
        read_timeout_secs: None,
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
        KernelBridge::execute_cell(service.as_ref(), &code, "", &cancelled, None)
    })
    .await
    .map_err(|error| error.to_string())?
}

#[tokio::test]
async fn bundled_python_skills_work_through_the_kernel() -> TestResult {
    let provider = Arc::new(ProviderStream::new(None));
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
    yi_runtime::history::register_grep(
        &mut registry,
        session.store_handle(),
        &yi_runtime::Wall::default(),
    );

    let service = Arc::new(KernelService::new(KernelServiceOptions {
        cwd: std::env::temp_dir(),
        home: std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_default(),
        session_dir: None,
        family_dir: None,
        host: Arc::new(registry),
        on_restore: None,
        on_boot: None,
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

    // The cell a refused read names runs as named, on an image under a name with a quote and a
    // backslash that a bare "{path}" literal breaks on (the 2026-09-11 v4 sweep's image reads).
    let dir = Scratch::new("yi-skills-attach")?;
    let image = dir.join("sch\"em\\atic.bmp");
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/fixtures/documents/checker.bmp"
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
    assert_eq!(attach_cell.result.attachments[0].mime_type, "image/jpeg");

    service.dispose().await;
    Ok(())
}

fn skill_dir(root: &std::path::Path, name: &str, frontmatter: &str) -> TestResult {
    let dir = root.join(name);
    std::fs::create_dir_all(&dir)?;
    std::fs::write(dir.join("SKILL.md"), frontmatter)?;
    Ok(())
}

fn service(cwd: PathBuf, home: PathBuf, host: HostRegistry) -> Arc<KernelService> {
    Arc::new(KernelService::new(KernelServiceOptions {
        cwd,
        home,
        session_dir: None,
        family_dir: None,
        host: Arc::new(host),
        on_restore: None,
        on_boot: None,
        sandbox: None,
        snapshot_key: None,
        per_session_state: false,
        cell_ceiling: None,
    }))
}

fn install_shipped(home: &std::path::Path, name: &str) -> Result<String, Box<dyn Error>> {
    let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills");
    let text = std::fs::read_to_string(shipped.join(name).join("SKILL.md"))?;
    skill_dir(&home.join(".yi/skills"), name, &text)?;
    Ok(text)
}

fn catalog_text(project: &std::path::Path, home: &std::path::Path) -> String {
    yi_runtime::skills_catalog(project, home, yi_runtime::Bytes(16_384))
        .map(|catalog| catalog.text)
        .unwrap_or_default()
}

fn read_skill(
    cwd: &std::path::Path,
    home: &std::path::Path,
    name: &str,
) -> Result<String, Box<dyn Error>> {
    let resolver = yi_runtime::fetch::Resolver::new(cwd.to_path_buf(), yi_runtime::Wall::default())
        .with_home(home.to_path_buf());
    let address = format!("yi://skills/{name}");
    Ok(resolver
        .fetch(&address.trim_end_matches('/').parse()?)?
        .text)
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
        catalog.text.contains("read(\"yi://skills/<name>\")"),
        "the catalog must say where the whole skill is: {}",
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
        catalog
            .text
            .contains("review: Verify finished work against its acceptance criteria with a…\n"),
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
    assert_eq!(
        full.text.matches(&format!("{}…\n", "x".repeat(59))).count(),
        40
    );

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
    assert!(!clipped.text.contains("(source budget"), "{}", clipped.text);

    let tight =
        yi_runtime::skills_catalog(&home, &home, yi_runtime::Bytes(2_048)).ok_or("no catalog")?;
    assert!(tight.truncated);
    assert!(tight.text.contains("more: "), "{}", tight.text);
    assert!(
        tight.text.contains("skill-39"),
        "the last name rides the more line: {}",
        tight.text
    );
    let floor =
        yi_runtime::skills_catalog(&home, &home, yi_runtime::Bytes(300)).ok_or("no catalog")?;
    let row = r#"(source budget 300); the rest: read("yi://skills/<name>") for one skill]"#;
    assert!(floor.text.ends_with(row), "{}", floor.text);
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
        on_boot: None,
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

/// Yi's `ripwire` index, the shipped file, stands in for ripwire's own skills: one catalog line
/// while ripwire is installed by yi, none otherwise, and the skills it points at are never listed.
/// A plugin that publishes no skills has no `skills/` directory, so its pointer skill needs the
/// install itself to appear, and must stay hidden without it.
#[test]
fn the_desktop_pointer_is_listed_only_when_its_skill_less_plugin_is_installed() -> TestResult {
    let root = Scratch::new("yi-skills-desktop")?;
    let (project, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&project)?;
    let index = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/yi/desktop");
    let shipped = home.join(".yi/skills/yi/desktop");
    std::fs::create_dir_all(&shipped)?;
    std::fs::copy(index.join("SKILL.md"), shipped.join("SKILL.md"))?;
    let catalog = || {
        yi_runtime::skills_catalog(&project, &home, yi_runtime::Bytes(16_384))
            .map(|catalog| catalog.text)
            .unwrap_or_default()
    };
    assert!(!catalog().contains("- desktop:"), "{}", catalog());
    let plugin = yi_tools::plugin::Plugin::named("agent-desktop").ok_or("no row")?;
    std::fs::create_dir_all(plugin.dir(&home))?;
    std::fs::write(plugin.dir(&home).join("agent-desktop"), "#!/bin/sh\n")?;
    assert!(catalog().contains("- desktop:"), "{}", catalog());
    Ok(())
}

#[test]
fn the_agent_browser_pointer_is_listed_only_when_its_plugin_is_installed() -> TestResult {
    let root = Scratch::new("yi-skills-agent-browser")?;
    let (project, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&project)?;
    let index =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/yi/agent-browser");
    let shipped = home.join(".yi/skills/yi/agent-browser");
    std::fs::create_dir_all(&shipped)?;
    std::fs::copy(index.join("SKILL.md"), shipped.join("SKILL.md"))?;
    let catalog = || {
        yi_runtime::skills_catalog(&project, &home, yi_runtime::Bytes(16_384))
            .map(|catalog| catalog.text)
            .unwrap_or_default()
    };
    assert!(!catalog().contains("- agent-browser:"), "{}", catalog());
    let plugin = yi_tools::plugin::Plugin::named("agent-browser").ok_or("no row")?;
    std::fs::create_dir_all(plugin.dir(&home))?;
    std::fs::write(plugin.dir(&home).join("agent-browser"), "#!/bin/sh\n")?;
    assert!(catalog().contains("- agent-browser:"), "{}", catalog());
    Ok(())
}

#[test]
fn the_ripwire_index_is_the_one_line_for_an_installed_plugins_skills() -> TestResult {
    let root = Scratch::new("yi-skills-plugin")?;
    let (project, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&project)?;
    let index = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/yi/ripwire");
    let shipped = home.join(".yi/skills/yi/ripwire");
    std::fs::create_dir_all(&shipped)?;
    std::fs::copy(index.join("SKILL.md"), shipped.join("SKILL.md"))?;
    let catalog = || {
        yi_runtime::skills_catalog(&project, &home, yi_runtime::Bytes(16_384))
            .map(|catalog| catalog.text)
            .unwrap_or_default()
    };
    assert!(!catalog().contains("- ripwire:"), "{}", catalog());
    let ripwire = yi_tools::plugin::Plugin::named("ripwire").ok_or("no ripwire row")?;
    std::fs::create_dir_all(ripwire.dir(&home))?;
    std::fs::write(ripwire.dir(&home).join("ripwire"), "#!/bin/sh\n")?;
    for skill in ripwire.skills {
        skill_dir(
            &ripwire.dir(&home).join("skills"),
            skill,
            &format!("---\nname: {skill}\ndescription: {skill} text.\n---\n"),
        )?;
    }
    let text = catalog();
    assert_eq!(text.matches("- ripwire").count(), 1, "{text}");
    assert!(
        text.contains("- ripwire: Which ripwire skill fits"),
        "{text}"
    );
    let found = yi_runtime::skills::discover(&project, &home);
    assert!(found.iter().any(|skill| skill.name == "ripwire-orient"));
    Ok(())
}

/// The index names exactly the skills the plugin row publishes, so a pin bump that renames or
/// adds one fails here rather than leaving a pointer to nothing.
#[test]
fn the_ripwire_index_names_every_published_skill_and_no_other() -> TestResult {
    let index =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/yi/ripwire/SKILL.md");
    let text = std::fs::read_to_string(index)?;
    let mut named: Vec<&str> = text
        .lines()
        .filter_map(|line| {
            line.strip_prefix("| ")?
                .rsplit_once("| `")?
                .1
                .strip_suffix("` |")
        })
        .collect();
    named.sort_unstable();
    let ripwire = yi_tools::plugin::Plugin::named("ripwire").ok_or("no ripwire row")?;
    let mut published = ripwire.skills.to_vec();
    published.sort_unstable();
    assert_eq!(named, published);
    let digest = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tools/tests/fixtures/ripwire")
        .join(format!("skills-{}.sha256", ripwire.version));
    let manifest = std::fs::read_to_string(digest)?;
    let mut shipped: Vec<&str> = manifest
        .lines()
        .filter_map(|line| line.split_once("  skills/")?.1.strip_suffix("/SKILL.md"))
        .collect();
    shipped.sort_unstable();
    assert_eq!(
        shipped, published,
        "the pinned manifest and the table disagree"
    );
    Ok(())
}

/// The skills the pinned ripwire release publishes are byte for byte the recorded ones, and the
/// installed copy is read by address; `just journeys` runs it after `yi plugin add ripwire`.
#[test]
#[ignore = "tier-2 journey: `just journeys`"]
fn the_installed_ripwire_skills_match_the_recorded_release() -> TestResult {
    let ripwire = yi_tools::plugin::Plugin::named("ripwire").ok_or("no ripwire row")?;
    let home = std::path::PathBuf::from(std::env::var_os("HOME").ok_or("no HOME")?);
    let dir = ripwire
        .skill_root(&home)
        .ok_or("ripwire's skills are not installed: run `yi plugin add ripwire`")?;
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../tools/tests/fixtures/ripwire")
        .join(format!("skills-{}.sha256", ripwire.version));
    let recorded = std::fs::read_to_string(fixture)?;
    for line in recorded.lines() {
        let (sha256, path) = line.split_once("  ").ok_or("not a sha256sum line")?;
        let relative = path.strip_prefix("skills/").ok_or("not under skills/")?;
        let bytes = std::fs::read(dir.join(relative))?;
        assert_eq!(yi_runtime::tarball::sha256_hex(&bytes), sha256, "{path}");
    }
    let resolver =
        yi_runtime::fetch::Resolver::new(home.clone(), yi_runtime::Wall::default()).with_home(home);
    let orient = resolver.fetch(&"yi://skills/ripwire-orient".parse()?)?;
    assert!(
        orient.text.contains("name: ripwire-orient"),
        "{}",
        orient.text
    );
    Ok(())
}

/// Dies with one repository's recipe offered in every other: a skill that names a path with
/// `within:` is listed, triggered, classified and read only where the repository holds that path.
#[test]
fn a_skill_within_a_repository_is_listed_only_there() -> TestResult {
    let home = Scratch::new("yi-skills-within")?;
    std::fs::create_dir_all(home.join(".yi/skills"))?;
    skill_dir(
        &home.join(".yi/skills"),
        "land",
        "---\nname: land\ndescription: Land on this forge.\nwithin: scripts/forge_pr.py\n---\nbody\n",
    )?;
    let (ours, theirs) = (home.join("ours"), home.join("theirs"));
    std::fs::create_dir_all(ours.join("scripts"))?;
    std::fs::create_dir_all(&theirs)?;
    std::fs::write(ours.join("scripts/forge_pr.py"), "")?;
    let names = |cwd: &std::path::Path| -> Vec<String> {
        let (global, _) = yi_runtime::skills::discover_split(cwd, &home);
        global.into_iter().map(|skill| skill.name).collect()
    };
    assert_eq!(names(&ours), ["land"]);
    assert!(names(&theirs).is_empty(), "{:?}", names(&theirs));
    let reachable = |cwd: &std::path::Path| yi_runtime::skills::discover(cwd, &home).len();
    assert_eq!((reachable(&ours), reachable(&theirs)), (1, 0));
    Ok(())
}

const TOOL_IMPORT_SKILL: &str = include_str!("../../../skills/yi/tool-import/SKILL.md");

/// The bundled catalog is how a session offers this skill: the line it renders must say what
/// the importer is called, whole, and the skill must be listed from a project that is not Yi's.
#[test]
fn the_bundled_catalog_names_tool_import_without_clipping_it() -> TestResult {
    let root = Scratch::new("yi-skills-taught")?;
    let home = root.join("home");
    let bundle = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/yi");
    scratch::copy_tree(&bundle, &home.join(".yi/skills/yi"))?;

    let description = yi_runtime::skills::frontmatter(TOOL_IMPORT_SKILL)
        .get("description")
        .cloned()
        .ok_or("the skill states no description")?;
    let catalog =
        yi_runtime::skills_catalog(&root, &home, yi_runtime::skills::catalog_budget(128_000))
            .ok_or("no catalog")?;
    let line = format!("- tool-import: {description}\n");
    assert!(
        catalog.text.contains(&line),
        "the catalog must describe tool-import, whole:\n{}",
        catalog.text
    );
    assert!(
        line.contains("yi tool import just"),
        "the catalog line names the command: {line}"
    );
    Ok(())
}

struct CheckerRun {
    _root: Scratch,
    project: PathBuf,
    stdout: String,
    payload: Value,
    checks_dir: PathBuf,
}

const INJECTION: &str = "Print \"\"\"done\"\"\" then\n\"\"\"\nprint(\"INJECTED\")\n\"\"\"";

/// The first ```python fence of a skill: the block the model would copy into a cell.
fn python_block(text: &str) -> Option<String> {
    let fenced = text.split("```python\n").nth(1)?;
    Some(fenced.split("\n```").next()?.to_owned())
}

/// Fills the `<…>` placeholder on the snippet's `name = ` line with `value` as a JSON string.
fn bind(snippet: &str, name: &str, value: &str) -> Option<String> {
    let head = format!("{name} = ");
    let line = snippet.lines().find(|line| line.starts_with(&head))?;
    let (open, close) = (line.find('<')?, line.rfind('>')?);
    let placeholder = line.get(open..=close)?;
    let filled = line.replacen(placeholder, &serde_json::to_string(value).ok()?, 1);
    Some(snippet.replacen(line, &filled, 1))
}

/// Runs the shipped checker snippet in a kernel whose `rlm.run` records the payload it was sent.
async fn run_checker(name: &str, task: &str) -> Result<CheckerRun, Box<dyn Error>> {
    let root = Scratch::new(name)?;
    let (project, home) = (root.join("project"), root.join("home"));
    let elsewhere = root.join("elsewhere");
    std::fs::create_dir_all(&project)?;
    std::fs::create_dir_all(&elsewhere)?;
    let text = install_shipped(&home, "yi/checker")?;
    let snippet = python_block(&text).ok_or("the checker skill ships no python snippet")?;
    let snippet = bind(&snippet, "task_text", task)
        .and_then(|snippet| bind(&snippet, "workspace", &project.to_string_lossy()))
        .ok_or("the snippet binds no task_text or workspace placeholder")?;
    let seen: Arc<std::sync::Mutex<Option<Value>>> = Arc::new(std::sync::Mutex::new(None));
    let mut registry = HostRegistry::default();
    {
        let seen = Arc::clone(&seen);
        registry.register("rlm.run", move |payload| {
            let seen = Arc::clone(&seen);
            let handle = json!({"rlm_child_id": "sub-checker", "name": "worker-1",
                "session_dir": "/tmp/sub-checker", "model": "faux/faux-1"});
            Box::pin(async move {
                *seen.lock().map_err(|_| "poisoned".to_owned())? = Some(Value::Object(payload));
                handle
                    .as_object()
                    .cloned()
                    .ok_or_else(|| "handle".to_owned())
            })
        });
    }
    let service = service(elsewhere, home, registry);
    let out = cell(&service, snippet)
        .await
        .map_err(|error| error.to_string())?;
    service.dispose().await;
    let stdout = out.result.stdout;
    assert!(
        stdout.contains("worker-1"),
        "the snippet must spawn its child and name it: {stdout} {}",
        out.result.stderr
    );
    let payload = seen
        .lock()
        .map_err(|_| "poisoned")?
        .clone()
        .ok_or("the snippet spawned nothing")?;
    let checks_dir = stdout
        .split_whitespace()
        .nth(1)
        .map(PathBuf::from)
        .ok_or("the snippet must print its checks_dir")?;
    Ok(CheckerRun {
        _root: root,
        project,
        stdout,
        payload,
        checks_dir,
    })
}

#[test]
fn the_checker_skill_is_listed_with_its_condition_and_readable() -> TestResult {
    let root = Scratch::new("yi-skills-checker-list")?;
    let (project, home) = (root.join("project"), root.join("home"));
    std::fs::create_dir_all(&project)?;
    install_shipped(&home, "yi/checker")?;
    let catalog = catalog_text(&project, &home);
    let line = catalog
        .lines()
        .find(|line| line.contains("- checker: "))
        .ok_or_else(|| format!("the checker skill must be a catalog line: {catalog}"))?;
    assert!(
        line.contains("interactive root or an allow rule"),
        "the clipped catalog line must carry the headless condition: {line}"
    );
    let whole = read_skill(&project, &home, "checker")?;
    assert!(whole.contains("name: checker"), "{whole}");
    Ok(())
}

#[tokio::test]
async fn the_checker_snippet_carries_the_task_text_as_data() -> TestResult {
    let run = run_checker("yi-skills-checker-inject", INJECTION).await?;
    assert!(
        !run.stdout.contains("INJECTED"),
        "a triple quote in the task text ran as Python: {}",
        run.stdout
    );
    let prompt = run
        .payload
        .get("prompt")
        .and_then(Value::as_str)
        .unwrap_or("");
    assert!(
        prompt.contains(INJECTION),
        "the brief must carry the task text unchanged: {prompt}"
    );
    Ok(())
}

#[test]
fn the_tool_import_skill_shows_the_schema_and_usage_the_model_acts_on() -> TestResult {
    for needle in [
        "yi tool import just [recipe ...]",
        "--schema",
        "always `exec`",
        "yi: skip",
        "next session",
        "another justfile",
    ] {
        assert!(
            TOOL_IMPORT_SKILL.contains(needle),
            "the skill no longer teaches {needle}"
        );
    }
    assert!(
        !TOOL_IMPORT_SKILL.contains("within:"),
        "a within: line hides the skill outside Yi's own repository"
    );
    Ok(())
}

#[tokio::test]
async fn the_checker_snippet_walls_history_and_the_workspace_but_not_checks_dir() -> TestResult {
    let run = run_checker("yi-skills-checker-walls", "Add a flag.").await?;
    let (project, payload) = (&run.project, &run.payload);
    let kwargs = payload
        .get("kwargs")
        .and_then(Value::as_object)
        .ok_or("no kwargs")?;
    let wall = yi_runtime::Wall::from_kwargs(kwargs, project)?;
    assert!(
        wall.denies_history(),
        "the snippet's spawn must wall history: {payload}"
    );
    let url: yi_types::url::Url = "history://parent/since/0".parse()?;
    let refusal = wall
        .check_url(&url, project)
        .ok_or("the wall must refuse a history read")?;
    assert!(refusal.contains("deny_url"), "{refusal}");

    let prompt = payload.get("prompt").and_then(Value::as_str).unwrap_or("");
    let checks = run.checks_dir.join("checks.py");
    assert!(
        prompt.contains(&checks.display().to_string()),
        "the brief must name the bound checks_dir: {prompt}"
    );
    let canonical = std::fs::canonicalize(project)?;
    assert!(
        !run.checks_dir.starts_with(project) && !run.checks_dir.starts_with(&canonical),
        "checks_dir must lie outside the workspace: {}",
        run.checks_dir.display()
    );
    let target = |path: &std::path::Path| {
        let mut args = serde_json::Map::new();
        args.insert("path".to_owned(), json!(path.to_string_lossy()));
        args
    };
    let attempt = project.join("src/lib.rs");
    for (tool, kind) in [
        ("read", yi_tools::ToolKind::Read),
        ("write", yi_tools::ToolKind::Write),
    ] {
        assert!(
            wall.check(tool, kind, &target(&attempt), project).is_some(),
            "the snippet's spawn must wall {tool} on the workspace: {payload}"
        );
    }
    assert_eq!(
        wall.check(
            "write",
            yi_tools::ToolKind::Write,
            &target(&checks),
            project
        ),
        None,
        "checks_dir must stay writable"
    );
    Ok(())
}

fn index(cwd: &std::path::Path, home: &std::path::Path) -> Result<String, Box<dyn Error>> {
    read_skill(cwd, home, "")
}

#[test]
fn the_index_names_only_what_the_catalog_lists_and_the_wall_admits() -> TestResult {
    let root = Scratch::new("yi-skills-index-rule")?;
    let (home, project) = (root.join("home"), root.join("project"));
    let skill = |dir: std::path::PathBuf, name: &str| {
        skill_dir(
            &dir,
            name,
            &format!("---\nname: {name}\ndescription: The {name} skill.\n---\nbody\n"),
        )
    };
    skill(home.join(".yi/skills"), "own")?;
    skill(home.join(".claude/skills"), "foreign")?;
    skill(project.join(".yi/skills"), "walled")?;
    let mut wall = yi_runtime::Wall::default();
    wall.deny_read.push(project.join(".yi/skills"));
    let resolver = yi_runtime::fetch::Resolver::new(project.clone(), wall).with_home(home.clone());
    let served = resolver.fetch(&"yi://skills".parse()?)?.text;
    assert!(served.contains("- own: The own skill."), "{served}");
    assert!(
        !served.contains("foreign"),
        "an unnamed global root stays unlisted: {served}"
    );
    assert!(
        !served.contains("walled"),
        "a walled skill is not named: {served}"
    );
    assert!(
        index(&project, &home)?.contains("- walled: "),
        "unwalled, the project skill lists"
    );
    Ok(())
}

#[test]
fn the_index_serves_every_bundled_skill_whole_and_only_where_it_applies() -> TestResult {
    let root = Scratch::new("yi-skills-index")?;
    let (home, project) = (root.join("home"), root.join("project"));
    std::fs::create_dir_all(&project)?;
    let shipped = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../skills/yi");
    yi_runtime::spec::copy_tree(&shipped, &home.join(".yi/skills/yi"))?;
    let wide = "Prüfe jede Änderung gegen den Plan, bevor du etwas für erledigt erklärst — immer, ohne Ausnahme.";
    skill_dir(
        &project.join(".yi/skills"),
        "prüfen",
        &format!("---\nname: prüfen\ndescription: {wide}\n---\nbody\n"),
    )?;
    let served = index(&project, &home)?;
    assert!(served.contains(&format!("- prüfen: {wide}\n")), "{served}");
    let read = std::fs::read_to_string(shipped.join("read/SKILL.md"))?;
    let tail = "or when an edit";
    assert!(
        read.contains(tail),
        "the read skill's description changed: {read}"
    );
    let line = (served.lines())
        .find(|line| line.starts_with("- read: "))
        .ok_or("the index lacks the read skill")?;
    assert!(
        line.contains(tail),
        "the description is whole, not clipped: {line}"
    );
    assert!(
        !served.contains("- gate: "),
        "gate is within the yi repository only: {served}"
    );
    std::fs::create_dir_all(project.join("scripts/guardrails"))?;
    std::fs::write(project.join("scripts/guardrails/check_guardrails.sh"), "")?;
    assert!(
        index(&project, &home)?.contains("- gate: "),
        "gate applies once its path exists"
    );
    assert!(!served.contains("skills described ("), "{served}");
    Ok(())
}

#[test]
fn an_empty_home_serves_an_index_that_says_where_skills_live() -> TestResult {
    let root = Scratch::new("yi-skills-index-empty")?;
    let served = index(&root.join("project"), &root.join("home"))?;
    assert!(served.starts_with("No skills found"), "{served}");
    Ok(())
}

#[test]
fn the_index_cut_names_its_cap_and_keeps_every_name() -> TestResult {
    let root = Scratch::new("yi-skills-index-cap")?;
    let home = root.join("home");
    let skills = home.join(".yi/skills");
    let write = |name: &str, description: &str| {
        skill_dir(
            &skills,
            name,
            &format!("---\nname: {name}\ndescription: {description}\n---\nbody\n"),
        )
    };
    for index in 0..30 {
        write(&format!("skill-{index:02}"), &"d".repeat(1_000))?;
    }
    let cap = yi_runtime::skills::CATALOG_CEILING.0;
    let short = index(&home, &home)?;
    assert!(short.len() < cap, "the fixture must start under the cap");
    write("skill-29", &"d".repeat(1_000 + cap - short.len()))?;
    let at = index(&home, &home)?;
    assert_eq!(at.len(), cap, "{}", at.len());
    assert!(
        !at.contains("skills described ("),
        "exactly at the cap nothing is cut"
    );
    write("skill-29", &"d".repeat(1_001 + cap - short.len()))?;
    let over = index(&home, &home)?;
    let row = r#"[… 29 of 30 skills described (yi://skills cap 32768); the rest: read("yi://skills/<name>") for each name below]"#;
    assert!(over.contains(&format!("{row}\nskill-29\n")), "{over}");
    assert!(over.len() <= cap, "{}", over.len());
    Ok(())
}

/// The binary ships `python/skills` under `~/.yi/python/skills`, which no skill root scans.
#[test]
fn every_skill_file_the_binary_ships_is_under_a_skill_root() -> TestResult {
    let scratch = Scratch::new("yi-shipped-skills")?;
    let home = scratch.home()?;
    let python = yi_kernel::bootstrap::unpack_embedded_python(&home)?;
    let reachable: std::collections::BTreeSet<PathBuf> = yi_runtime::skills::discover(&home, &home)
        .into_iter()
        .filter_map(|skill| skill.path.canonicalize().ok())
        .collect();
    let mut shipped = Vec::new();
    shipped_skill_files(&python, &mut shipped)?;
    let unreachable: Vec<&PathBuf> = shipped
        .iter()
        .filter(|path| {
            !path
                .canonicalize()
                .is_ok_and(|path| reachable.contains(&path))
        })
        .collect();
    assert!(
        unreachable.is_empty(),
        "no skill root scans these, so no model reads them: {unreachable:?}"
    );
    Ok(())
}

fn shipped_skill_files(dir: &std::path::Path, found: &mut Vec<PathBuf>) -> std::io::Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let path = entry?.path();
        if path.is_dir() {
            shipped_skill_files(&path, found)?;
        } else if path.file_name().is_some_and(|name| name == "SKILL.md") {
            found.push(path);
        }
    }
    Ok(())
}
