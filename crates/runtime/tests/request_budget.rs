#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::anthropic::{AnthropicOptions, Thinking, build_params};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_ai::openai::{self, OpenAiOptions};
use yi_runtime::{
    AgentSession, PermissionMode, ProviderStream, SessionConfig, identity_fragment, mode_fragment,
};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{LlmContext, Model, ModelCost, ToolDef};

type TestResult = Result<(), Box<dyn Error>>;

/// A fixed model, not a catalog lookup: the budget measures Yi's own prompt and
/// tool table, and must not move when a bundled model's metadata changes.
fn model() -> Model {
    let zero = || serde_json::Number::from(0u64);
    Model {
        id: "claude-opus-4-5".to_owned(),
        name: "Opus".to_owned(),
        api: "anthropic-messages".to_owned(),
        provider: "anthropic".to_owned(),
        base_url: "https://api.anthropic.com".to_owned(),
        reasoning: true,
        input: vec!["text".to_owned()],
        cost: ModelCost {
            input: zero(),
            output: zero(),
            cache_read: zero(),
            cache_write: zero(),
            tiers: None,
        },
        context_window: 200_000,
        max_tokens: 32_000,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

fn openai_model() -> Model {
    Model {
        id: "gpt-5".to_owned(),
        name: "GPT-5".to_owned(),
        api: "openai-completions".to_owned(),
        provider: "openai".to_owned(),
        base_url: "https://api.openai.com/v1".to_owned(),
        ..model()
    }
}

/// The fetched-catalog shape (#742): no cache flag exists, and the route alone earns the marks.
fn openrouter_model() -> Model {
    Model {
        id: "anthropic/claude-haiku-4.5".to_owned(),
        provider: "openrouter".to_owned(),
        base_url: "https://openrouter.ai/api/v1".to_owned(),
        ..openai_model()
    }
}

fn options() -> AnthropicOptions {
    AnthropicOptions {
        thinking: Thinking::Off,
        ..AnthropicOptions::default()
    }
}

/// The `documentFormats` the kernel venv's anydoc 0.2.x wheel reports, recorded
/// 2026-09-11, so the read tool's document clause joins the locked table on a
/// machine with no venv.
///
/// ponytail: the lock pins this recorded list, not the live wheel — an anydoc
/// upgrade that changes the real formats drifts silently until someone re-probes
/// the venv and edits the list. The upgrade path is a live-lane test that builds
/// the venv and asserts the wheel's list equals this one.
const RECORDED_DOCUMENT_FORMATS: [&str; 11] = [
    "doc",
    "docx (docm)",
    "odt",
    "pdf",
    "ppt (pps, pot)",
    "pptx (ppsx, ppsm, pptm)",
    "rtf",
    "epub",
    "xlsx (xls, xlsm, xlsb)",
    "ods",
    "odp",
];

/// The table a real session registers, built the way the CLI builds it:
/// `session_tools` for the builtins and `attach_runtime` for ipython, ask_user,
/// plan and todo, over scratch dirs so nothing touches the real HOME. Pricing
/// only the builtins would leave the largest tools in the prefix invisible to
/// the gate that exists to catch prefix growth, and the surface lock must pin
/// what a model call can actually name.
fn session_tool_defs() -> Result<Vec<ToolDef>, Box<dyn Error>> {
    use std::path::PathBuf;
    use std::sync::Arc;

    // attach_runtime spawns nothing with prewarm off, but lanes and hooks touch
    // tokio primitives that insist a reactor exists.
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let _reactor = runtime.enter();
    let root = Scratch::new("yi-surface")?;
    let (cwd, home) = (root.join("cwd"), root.join("home"));
    std::fs::create_dir_all(&cwd)?;
    std::fs::create_dir_all(&home)?;
    let fixed = yi_tools::Documents::fixed(
        home.clone(),
        yi_tools::Converter {
            python: PathBuf::new(),
            formats: RECORDED_DOCUMENT_FORMATS
                .iter()
                .map(|&format| format.to_owned())
                .collect(),
        },
    );
    let provider = Arc::new(ProviderStream::new(None));
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: String::new(),
            model: model(),
            thinking_level: None,
            tool_execution: yi_runtime::ExecutionMode::Sequential,
        },
        Arc::clone(&provider),
    );
    yi_runtime::attach_runtime(
        &mut session,
        yi_runtime::RuntimeWiring {
            provider,
            system_prompt: String::new(),
            tool_execution: yi_runtime::ExecutionMode::Sequential,
            cwd: cwd.clone(),
            home: home.clone(),
            lane_slots: 1,
            broker: None,
            tools: Arc::new(move || yi_runtime::session_tools(false, Some(fixed.clone()), None)),
            depth: 0,
            max_depth: 1,
            rlm_dir: root.join("rlm"),
            family_dir: None,
            summarizer: None,
            advisor: None,
            auto_review: None,
            plan_stale_turns: None,
            plans_dir: Some(root.join("plans")),
            parent_link: None,
            wall: yi_runtime::Wall::default(),
            auto_background: None,
            deadline: None,
            kernel_prewarm: false,
            mcp_read: None,
            sessions_dir: Some(root.join("sessions")),
            kernels: yi_runtime::fetch::KernelServiceMap::new(),
        },
    );
    let defs = session
        .tools()
        .iter()
        .map(|tool| tool.definition())
        .collect();
    Ok(defs)
}

fn tool_defs() -> Result<Vec<ToolDef>, Box<dyn Error>> {
    static TABLE: std::sync::OnceLock<Result<Vec<ToolDef>, String>> = std::sync::OnceLock::new();
    Ok(TABLE
        .get_or_init(|| session_tool_defs().map_err(|err| err.to_string()))
        .clone()?)
}

/// One lock line per key the model reads: each registered tool's description
/// and schema, each kernel extra a hint can name, and the identity fragment
/// that says what each tool is for. check_request_budget.py hashes the texts.
fn surface_lines() -> Result<Vec<String>, Box<dyn Error>> {
    let mut lines = Vec::new();
    for def in tool_defs()? {
        let schema = serde_json::to_string(&def.parameters)?;
        let text = format!("{}\n{schema}", def.description);
        lines.push(json!({"key": format!("tool:{}", def.name), "text": text}).to_string());
    }
    for arg in yi_kernel::bootstrap::DEFAULT_RLM_EXTRA_UV_ARGS {
        let package = arg.split(['<', '>', '=', '!', ' ']).next().unwrap_or(arg);
        lines.push(json!({"key": format!("extra:{package}"), "text": package}).to_string());
    }
    lines.push(json!({"key": "prompt:identity", "text": identity_fragment()}).to_string());
    Ok(lines)
}

/// strict_tools.rs in yi-ai sends this table to the providers, so its fixture is regenerated
/// from here whenever it drifts: the test dies with a stale fixture, naming how to refresh it.
fn regenerate_tool_schemas_fixture() -> TestResult {
    let defs = tool_defs()?;
    let tools: Vec<Value> = defs
        .iter()
        .map(|def| {
            json!({
                "name": def.name,
                "description": def.description,
                "parameters": def.parameters,
            })
        })
        .collect();
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../ai/tests/fixtures/tool_schemas_2026-10-03.json");
    let current = serde_json::to_string_pretty(&tools)?;
    let recorded = std::fs::read_to_string(&path)?;
    let live = serde_json::to_string(&tools)?;
    let matches = serde_json::from_str::<Value>(&recorded)
        .ok()
        .and_then(|recorded| serde_json::to_string(&recorded).ok())
        .is_some_and(|recorded| recorded == live);
    if matches {
        return Ok(());
    }
    std::fs::write(&path, format!("{current}\n"))?;
    Err(format!(
        "the tool-schemas fixture a strict_tools test reads was stale: it now holds the \
         schemas the live session registers; review the diff, rerun this test, revert it to \
         pass: {path:?}"
    )
    .into())
}

#[test]
fn the_tool_schemas_fixture_matches_the_live_session() -> TestResult {
    regenerate_tool_schemas_fixture()
}

/// The skills catalog is deliberately excluded: it is assembled from the
/// machine's global and project roots, so including it would make the budget
/// depend on what the runner has installed.
fn system_prompt() -> String {
    format!(
        "{}\n{}\n{}",
        identity_fragment(),
        yi_runtime::doctrine_fragment(),
        mode_fragment(PermissionMode::Ask)
    )
}

fn user(text: &str) -> AgentMessage {
    AgentMessage::user_input(UserContent::Text(text.to_owned()), 0)
}

fn assistant_call(id: &str, name: &str, arguments: Map<String, Value>) -> AgentMessage {
    faux_assistant_message(
        vec![faux_tool_call(id, name, arguments)],
        StopReason::ToolUse,
    )
}

fn tool_result(id: &str, name: &str, text: &str) -> AgentMessage {
    AgentMessage::ToolResult {
        tool_call_id: id.to_owned(),
        tool_name: name.to_owned(),
        content: vec![Content::Text {
            text: text.to_owned(),
            text_signature: None,
        }],
        details: None,
        usage: None,
        added_tool_names: None,
        is_error: false,
        timestamp: 0,
    }
}

fn context(messages: Vec<AgentMessage>) -> Result<LlmContext, Box<dyn Error>> {
    Ok(LlmContext {
        cache_ttl: yi_types::model::Ttl::Min5,
        system_prompt: system_prompt(),
        messages,
        transient: Vec::new(),
        schema: None,
        shared_through: None,
        reuse: yi_types::model::Reuse::Loop,
        tools: Some(tool_defs()?),
        tool_choice: None,
    })
}

fn first_turn() -> Vec<AgentMessage> {
    vec![user("read src/lib.rs and tell me what it exports")]
}

fn second_turn() -> Vec<AgentMessage> {
    let mut arguments = Map::new();
    arguments.insert("path".to_owned(), json!("src/lib.rs"));
    let mut messages = first_turn();
    messages.push(assistant_call("call-1", "read", arguments));
    messages.push(tool_result("call-1", "read", "pub mod advisor;"));
    messages.push(user("now check the tests"));
    messages
}

fn prefix_bytes(params: &Value) -> Result<usize, Box<dyn Error>> {
    let system = serde_json::to_string(params.get("system").unwrap_or(&Value::Null))?;
    let tools = serde_json::to_string(params.get("tools").unwrap_or(&Value::Null))?;
    Ok(system.len().saturating_add(tools.len()))
}

/// The breakpoint moves to the newest message every turn and is not part of the prefix hash.
/// Nothing else is forgiven: a plain string that became a part to carry a mark is a different
/// prompt once OpenRouter merges it with the user message beside it (#742, live: request 3
/// read 13,900 of 22,373 tokens).
fn strip_cache_control(value: &mut Value) {
    match value {
        Value::Object(map) => {
            map.remove("cache_control");
            for nested in map.values_mut() {
                strip_cache_control(nested);
            }
        }
        Value::Array(items) => {
            for item in items {
                strip_cache_control(item);
            }
        }
        _ => {}
    }
}

/// Every message the second turn inherits, serialized as the first turn sent
/// it. The breakpoint marker is dropped: it moves to the newest message every
/// turn and the documented cache key excludes it.
fn message_prefix(params: &Value) -> Result<Vec<String>, Box<dyn Error>> {
    let mut messages = params
        .get("messages")
        .cloned()
        .unwrap_or_else(|| Value::Array(Vec::new()));
    strip_cache_control(&mut messages);
    let items = messages.as_array().ok_or("messages is not an array")?;
    items
        .iter()
        .map(|message| Ok(serde_json::to_string(message)?))
        .collect()
}

/// A provider caches on an exact content prefix, so a request that re-renders
/// anything the previous turn already sent silently re-bills the whole
/// conversation while producing identical output — invisible to every other
/// gate. This is what D51 was.
fn assert_prefix_survives_a_turn(first: &Value, second: &Value) -> TestResult {
    assert_eq!(
        serde_json::to_string(first.get("system").unwrap_or(&Value::Null))?,
        serde_json::to_string(second.get("system").unwrap_or(&Value::Null))?,
        "the system block must not change between turns"
    );
    assert_eq!(
        serde_json::to_string(first.get("tools").unwrap_or(&Value::Null))?,
        serde_json::to_string(second.get("tools").unwrap_or(&Value::Null))?,
        "the tool table must not change between turns"
    );
    let (before, after) = (message_prefix(first)?, message_prefix(second)?);
    assert!(
        after.len() > before.len(),
        "the second turn must carry the first turn's messages plus its own"
    );
    for (index, sent) in before.iter().enumerate() {
        assert_eq!(
            Some(sent),
            after.get(index),
            "message {index} was re-rendered by a later turn (D51)"
        );
    }
    Ok(())
}

#[test]
fn the_anthropic_cached_prefix_survives_a_turn() -> TestResult {
    let model = model();
    let options = options();
    assert_prefix_survives_a_turn(
        &build_params(&model, &context(first_turn())?, &options),
        &build_params(&model, &context(second_turn())?, &options),
    )
}

/// OpenAI caches automatically on the same content prefix, and carries the
/// system prompt as the first message rather than its own field, so the same
/// check covers one more block there.
#[test]
fn the_openai_cached_prefix_survives_a_turn() -> TestResult {
    let model = openai_model();
    let options = OpenAiOptions {
        session_id: Some("session-1".to_owned()),
        ..OpenAiOptions::default()
    };
    assert_prefix_survives_a_turn(
        &openai::build_params(&model, &context(first_turn())?, &options),
        &openai::build_params(&model, &context(second_turn())?, &options),
    )
}

/// An Anthropic model behind OpenRouter marks its system block and newest message, never the
/// root, so the check is OpenAI's plus both marks on both turns and no OpenAI-only key.
#[test]
fn the_openrouter_cached_prefix_survives_a_turn() -> TestResult {
    let model = openrouter_model();
    let options = OpenAiOptions {
        session_id: Some("session-1".to_owned()),
        ..OpenAiOptions::default()
    };
    let first = openai::build_params(&model, &context(first_turn())?, &options);
    let second = openai::build_params(&model, &context(second_turn())?, &options);
    for params in [&first, &second] {
        let messages = params["messages"].as_array().ok_or("messages")?;
        let marked = |message: &Value| message["content"][0]["cache_control"].is_object();
        assert!(params.get("cache_control").is_none(), "{params}");
        assert!(messages.first().is_some_and(marked), "{params}");
        assert!(messages.last().is_some_and(marked), "{params}");
        assert!(params.get("prompt_cache_key").is_none());
    }
    assert_prefix_survives_a_turn(&first, &second)
}

/// The invariant the whole cache layout rests on: with no attach between two
/// requests, every cached block is byte-identical, and an attach rebuilds only
/// the block it landed in. The universal prefix (block 0) never moves, which is
/// what a fan-out of children reads.
#[test]
fn the_assembled_prefix_is_stable_across_a_turn_and_a_yard_change() -> TestResult {
    use yi_runtime::ext::{PromptState, Rank, Slot, Trust};
    use yi_types::model::SYSTEM_BLOCK_SEPARATOR;

    let mut state = PromptState::default();
    state.attach(
        Slot::new(Rank::Identity, "identity"),
        identity_fragment().to_owned(),
    );
    state.attach(
        Slot::new(Rank::Doctrine, "doctrine"),
        yi_runtime::doctrine_fragment().to_owned(),
    );
    state.attach(
        Slot::new(Rank::Mode, "permission"),
        mode_fragment(PermissionMode::Auto).to_owned(),
    );
    state.attach_external("AGENTS.md", Trust::Untrusted, "run the repo's own gate");

    let blocks = |state: &PromptState| -> Vec<String> {
        state
            .assemble()
            .split(SYSTEM_BLOCK_SEPARATOR)
            .map(str::to_owned)
            .collect()
    };
    let before = blocks(&state);
    assert_eq!(before.len(), 3, "universal, trusted, yard");
    assert_eq!(
        before,
        blocks(&state),
        "assembling the same state twice must produce the same bytes"
    );

    state.attach_external("AGENTS.md", Trust::Untrusted, "the repository changed");
    let after_yard = blocks(&state);
    assert_eq!(before[0], after_yard[0], "block 0 survives a yard change");
    assert_eq!(before[1], after_yard[1], "block 1 survives a yard change");
    assert_ne!(before[2], after_yard[2]);

    state.attach(Slot::new(Rank::Protocol, "orchestrate"), "PLAN".to_owned());
    let after_attach = blocks(&state);
    assert_eq!(
        before[0], after_attach[0],
        "a mid-session attach never rebuilds the universal prefix"
    );
    assert_ne!(before[1], after_attach[1]);
    Ok(())
}

/// chrono is banned, so the civil date is derived from the epoch by hand
/// (Howard Hinnant's civil_from_days).
fn iso_date(epoch_secs: i64) -> String {
    let days = epoch_secs.div_euclid(86_400).saturating_add(719_468);
    let era = days.div_euclid(146_097);
    let doe = days.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{day:02}")
}

/// How much company a marker needs before a hit counts as residue.
#[derive(Clone, Copy)]
enum Bound {
    /// A path or a date is distinctive enough that any occurrence is residue.
    Anywhere,
    /// A pid or a 7-char sha can coincide with prompt text, so no alphanumeric
    /// may sit on either side of the hit.
    Word,
    /// Incident: a username or hostname is often an ordinary English word
    /// (agent, root, build) and block 0 says "coding agent", so a hit counts
    /// only beside a path separator or an @ — the shapes residue takes.
    Neighboured,
}

fn residue(block: &str, needle: &str, bound: Bound) -> bool {
    let alnum = |ch: Option<char>| ch.is_some_and(|c: char| c.is_ascii_alphanumeric());
    let joins = |ch: Option<char>| ch.is_some_and(|c| c == '/' || c == '\\' || c == '@');
    !needle.is_empty()
        && block.match_indices(needle).any(|(at, hit)| {
            let before = block[..at].chars().next_back();
            let after = block[at.saturating_add(hit.len())..].chars().next();
            match bound {
                Bound::Anywhere => true,
                Bound::Word => !alnum(before) && !alnum(after),
                Bound::Neighboured => {
                    !alnum(before) && !alnum(after) && (joins(before) || joins(after))
                }
            }
        })
}

#[expect(
    clippy::disallowed_methods,
    reason = "the hostname and git-ref markers only exist outside the process; nothing in-tree reports them"
)]
fn command_output(program: &str, args: &[&str]) -> Option<String> {
    let out = std::process::Command::new(program)
        .args(args)
        .output()
        .ok()?;
    let text = String::from_utf8(out.stdout).ok()?.trim().to_owned();
    (out.status.success() && !text.is_empty()).then_some(text)
}

/// Block 0 of a fully installed host: the universal prefix a fan-out of
/// children reads, assembled after the session-start effects have fired.
fn frozen_block(cwd: &std::path::Path, home: &std::path::Path) -> Result<String, Box<dyn Error>> {
    use yi_runtime::ext::{ExtOptions, install};
    use yi_types::model::SYSTEM_BLOCK_SEPARATOR;

    std::fs::create_dir_all(cwd)?;
    std::fs::create_dir_all(home)?;
    let mut host = install(ExtOptions {
        cwd: cwd.to_path_buf(),
        home: home.to_path_buf(),
        mode: PermissionMode::Ask,
        user_system: String::new(),
        schema_instruction: None,
        context_window: 128_000,
        global_skills: Vec::new(),
    });
    host.start(None, false);
    Ok(host
        .system_prompt()
        .split(SYSTEM_BLOCK_SEPARATOR)
        .next()
        .unwrap_or_default()
        .to_owned())
}

/// Block 0 only: block 1 is machine-dependent by design (the skills catalog)
/// and block 2 is the repository's own text. Two renders in one process share
/// every ambient value, so invariance and a residue scan both have to run.
#[test]
fn the_frozen_prefix_is_location_invariant_and_residue_free() -> TestResult {
    let root = Scratch::new("yi-frozen")?;
    let (cwd_a, home_a) = (root.join("alpha/work"), root.join("alpha/dwelling"));
    let (cwd_b, home_b) = (root.join("beta/elsewhere"), root.join("beta/abode"));
    let a = frozen_block(&cwd_a, &home_a)?;
    let b = frozen_block(&cwd_b, &home_b)?;

    let mut violations: Vec<String> = Vec::new();
    if a != b {
        let at = a.bytes().zip(b.bytes()).take_while(|(x, y)| x == y).count();
        let window = |text: &str| {
            let end = text.len().min(at.saturating_add(60));
            text.get(at..end).unwrap_or(text).to_owned()
        };
        violations.push(format!(
            "block 0 is not location-invariant: first difference at byte {at}\n  a: {:?}\n  b: {:?}",
            window(&a),
            window(&b)
        ));
    }

    let now = i64::try_from(yi_session::now_ms() / 1_000)?;
    let shown = |path: &std::path::Path| path.display().to_string();
    let mut markers: Vec<(String, String, Bound)> = vec![
        ("cwd (pair a)".to_owned(), shown(&cwd_a), Bound::Anywhere),
        ("home (pair a)".to_owned(), shown(&home_a), Bound::Anywhere),
        ("cwd (pair b)".to_owned(), shown(&cwd_b), Bound::Anywhere),
        ("home (pair b)".to_owned(), shown(&home_b), Bound::Anywhere),
        (
            "pid".to_owned(),
            std::process::id().to_string(),
            Bound::Word,
        ),
        ("date (utc)".to_owned(), iso_date(now), Bound::Anywhere),
        (
            "date (utc-1d)".to_owned(),
            iso_date(now.saturating_sub(86_400)),
            Bound::Anywhere,
        ),
        (
            "date (utc+1d)".to_owned(),
            iso_date(now.saturating_add(86_400)),
            Bound::Anywhere,
        ),
    ];
    for (var, bound) in [
        ("HOME", Bound::Word),
        ("USER", Bound::Neighboured),
        ("LOGNAME", Bound::Neighboured),
    ] {
        match std::env::var(var) {
            Ok(value) if value.len() >= 3 => markers.push((format!("${var}"), value, bound)),
            _ => println!("SKIP ${var} marker: unset or too short to bound"),
        }
    }
    match command_output("hostname", &[]) {
        Some(name) => markers.push(("hostname".to_owned(), name, Bound::Neighboured)),
        None => println!("SKIP hostname marker: the `hostname` command is unavailable"),
    }
    match command_output(
        "git",
        &["-C", env!("CARGO_MANIFEST_DIR"), "rev-parse", "HEAD"],
    ) {
        Some(sha) => {
            let short: String = sha.chars().take(7).collect();
            markers.push(("git HEAD (short)".to_owned(), short, Bound::Word));
            markers.push(("git HEAD".to_owned(), sha, Bound::Word));
        }
        None => println!("SKIP git-ref marker: `git rev-parse HEAD` is unavailable"),
    }

    for (label, needle, bound) in &markers {
        if residue(&a, needle, *bound) {
            violations.push(format!(
                "block 0 carries ambient residue: {label} = {needle:?}"
            ));
        }
    }
    if violations.is_empty() {
        return Ok(());
    }
    Err(violations.join("\n").into())
}

/// Incident: a per-session fence nonce sat ahead of the tool table, so a new session reused
/// at most about 7k cached tokens of another's prefix. Same repo and HOME, same bytes.
#[test]
fn two_fresh_sessions_send_the_same_system_prompt_and_tools() -> TestResult {
    use yi_runtime::ext::{ExtOptions, install};

    let root = Scratch::new("yi-twin")?;
    let (cwd, home) = (root.join("repo"), root.join("home"));
    std::fs::create_dir_all(cwd.join(".git"))?;
    std::fs::create_dir_all(&home)?;
    std::fs::write(cwd.join("AGENTS.md"), "run the repo's own gate\n")?;
    let session = || -> Result<(String, String), Box<dyn Error>> {
        let mut host = install(ExtOptions {
            cwd: cwd.clone(),
            home: home.clone(),
            mode: PermissionMode::Auto,
            user_system: String::new(),
            schema_instruction: None,
            context_window: 128_000,
            global_skills: Vec::new(),
        });
        host.start(None, false);
        std::thread::sleep(std::time::Duration::from_millis(2));
        Ok((
            host.system_prompt(),
            serde_json::to_string(&session_tool_defs()?)?,
        ))
    };
    let (first, second) = (session()?, session()?);
    assert!(first.0.contains("<<<yi-external "), "{}", first.0);
    assert_eq!(first, second);
    Ok(())
}

/// Read by scripts/guardrails/check_request_budget.py, which owns the ratchet
/// and the surface lock.
#[test]
fn report_the_prefix_size() -> TestResult {
    let params = build_params(&model(), &context(first_turn())?, &options());
    let system = serde_json::to_string(params.get("system").unwrap_or(&Value::Null))?;
    let tools = serde_json::to_string(params.get("tools").unwrap_or(&Value::Null))?;
    println!(
        "REQUEST_PREFIX system={} tools={} total={}",
        system.len(),
        tools.len(),
        prefix_bytes(&params)?
    );
    for line in surface_lines()? {
        println!("TOOL_SURFACE {line}");
    }
    Ok(())
}

/// Body indexes of the history messages carrying a mark, the system message left out.
fn marked_history(params: &Value) -> Vec<usize> {
    params["messages"]
        .as_array()
        .into_iter()
        .flatten()
        .enumerate()
        .filter(|(_, message)| !matches!(message["role"].as_str(), Some("system" | "developer")))
        .filter(|(_, message)| {
            message["content"]
                .as_array()
                .is_some_and(|parts| parts.iter().any(|part| part["cache_control"].is_object()))
        })
        .map(|(index, _)| index)
        .collect()
}

/// Pops the environment off a rendered body: it is the last message, and no part of it
/// carries a mark.
fn strip_env(params: &yi_ai::breakpoints::Encoded, turn: u32) -> Result<Value, Box<dyn Error>> {
    let mut out = params.clone().into_value();
    let messages = out["messages"].as_array_mut().ok_or("messages")?;
    let last = messages.pop().ok_or("no messages")?;
    let text = last["content"]
        .as_str()
        .or_else(|| last["content"][0]["text"].as_str())
        .unwrap_or("");
    assert!(text.contains(&format!("turn: {turn}")), "{last}");
    let marked = last["content"]
        .as_array()
        .is_some_and(|parts| parts.iter().any(|part| part["cache_control"].is_object()));
    assert!(
        !marked,
        "the environment block is never a breakpoint: {last}"
    );
    Ok(out)
}

/// The D51 invariant over a scripted tool loop, on both dialects: the environment rides
/// `transient` and renders last and bare, every history message request k sent is byte-
/// identical in request k+1, and request k+1's previous-tail mark sits exactly where request
/// k's tail mark was, so it is a read and never a write (#743).
#[test]
fn a_tool_loop_keeps_its_prefix_and_reads_the_previous_tail_on_both_dialects() -> TestResult {
    let env = |turn: u32| {
        user(&format!(
            "<environment>\ncwd: /x\nturn: {turn}\n</environment>"
        ))
    };
    let mut third = second_turn();
    let last_user = third.pop().ok_or("second turn ends with a user")?;
    third.push(faux_assistant_message(
        vec![faux_text("it exports advisor")],
        StopReason::Stop,
    ));
    third.push(last_user);
    let mut second = second_turn();
    second.truncate(3);
    let requests: Vec<LlmContext> = [first_turn(), second, third]
        .into_iter()
        .zip(1..)
        .map(|(messages, turn)| {
            let mut ctx = context(messages)?;
            ctx.transient = vec![env(turn)];
            Ok::<_, Box<dyn Error>>(ctx)
        })
        .collect::<Result<_, _>>()?;
    let anthropic: Vec<yi_ai::breakpoints::Encoded> = requests
        .iter()
        .map(|ctx| build_params(&model(), ctx, &options()))
        .collect();
    let openrouter: Vec<yi_ai::breakpoints::Encoded> = requests
        .iter()
        .map(|ctx| openai::build_params(&openrouter_model(), ctx, &OpenAiOptions::default()))
        .collect();
    for (dialect, bodies) in [("anthropic", anthropic), ("openrouter", openrouter)] {
        let prefixes: Vec<Value> = bodies
            .iter()
            .zip(1..)
            .map(|(body, turn)| strip_env(body, turn))
            .collect::<Result<_, _>>()?;
        for pair in prefixes.windows(2) {
            let (earlier, later) = (&pair[0], &pair[1]);
            assert_prefix_survives_a_turn(earlier, later)?;
            let tail = marked_history(earlier);
            let next = marked_history(later);
            assert_eq!(
                tail.last(),
                next.get(next.len().wrapping_sub(2)),
                "{dialect}: the previous tail is re-marked where it was: {tail:?} then {next:?}"
            );
            assert!(
                next.last().is_some_and(|last| Some(last) > tail.last()),
                "{dialect}: the tail moves forward: {tail:?} then {next:?}"
            );
        }
    }
    Ok(())
}
