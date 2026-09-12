#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::error::Error;

use serde_json::{Map, Value, json};
use yi_ai::anthropic::{AnthropicOptions, Thinking, build_params};
use yi_ai::faux::{faux_assistant_message, faux_tool_call};
use yi_ai::openai::{self, OpenAiOptions};
use yi_runtime::{PermissionMode, builtin_tools, identity_fragment, mode_fragment};
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

/// The catalog flag, not the provider, is what earns a root breakpoint.
fn openrouter_model() -> Model {
    Model {
        id: "anthropic/claude-haiku-4.5".to_owned(),
        provider: "openrouter".to_owned(),
        base_url: "https://openrouter.ai/api/v1".to_owned(),
        compat: Some(json!({"cacheControlFormat": "anthropic", "thinkingFormat": "openrouter"})),
        ..openai_model()
    }
}

fn options() -> AnthropicOptions {
    AnthropicOptions {
        thinking: Thinking::Off,
        cache: true,
        ..AnthropicOptions::default()
    }
}

/// Invariant: the plan tool is wired per session rather than into the builtin
/// list, so pricing only the builtins would leave the largest single tool in
/// the prefix invisible to the gate that exists to catch prefix growth.
fn tool_defs() -> Vec<ToolDef> {
    builtin_tools()
        .iter()
        .map(|tool| ToolDef {
            name: tool.name().to_owned(),
            description: tool.description().to_owned(),
            parameters: tool.schema(),
            freeform: None,
        })
        .chain([
            ToolDef {
                name: "plan".to_owned(),
                description: yi_runtime::plan::tool::DESCRIPTION.to_owned(),
                parameters: yi_runtime::plan::tool::schema(),
                freeform: None,
            },
            ToolDef {
                name: yi_runtime::todo::tool::NAME.to_owned(),
                description: yi_runtime::todo::tool::DESCRIPTION.to_owned(),
                parameters: yi_runtime::todo::tool::schema(),
                freeform: None,
            },
        ])
        .collect()
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
    AgentMessage::host_user(UserContent::Text(text.to_owned()), 0)
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

fn context(messages: Vec<AgentMessage>) -> LlmContext {
    LlmContext {
        system_prompt: system_prompt(),
        messages,
        tools: Some(tool_defs()),
        tool_choice: None,
    }
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

/// The breakpoint moves to the newest message every turn and is not part of
/// the prefix hash, so it is the one key a stability check must ignore.
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
        &build_params(&model, &context(first_turn()), &options),
        &build_params(&model, &context(second_turn()), &options),
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
        &openai::build_params(&model, &context(first_turn()), &options),
        &openai::build_params(&model, &context(second_turn()), &options),
    )
}

/// An Anthropic model behind OpenRouter gets one root breakpoint that OpenRouter
/// moves to the newest block itself, so the check is the same as OpenAI's plus
/// the marker being present on both turns and no OpenAI-only key leaking in.
#[test]
fn the_openrouter_cached_prefix_survives_a_turn() -> TestResult {
    let model = openrouter_model();
    let options = OpenAiOptions {
        session_id: Some("session-1".to_owned()),
        ..OpenAiOptions::default()
    };
    let first = openai::build_params(&model, &context(first_turn()), &options);
    let second = openai::build_params(&model, &context(second_turn()), &options);
    for params in [&first, &second] {
        assert_eq!(params["cache_control"], json!({"type": "ephemeral"}));
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

    let mut state = PromptState::new("cafe1234".to_owned());
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
/// and block 2 carries a per-session nonce. Two renders in one process share
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

/// Read by scripts/guardrails/check_request_budget.py, which owns the ratchet.
#[test]
fn report_the_prefix_size() -> TestResult {
    let params = build_params(&model(), &context(first_turn()), &options());
    let system = serde_json::to_string(params.get("system").unwrap_or(&Value::Null))?;
    let tools = serde_json::to_string(params.get("tools").unwrap_or(&Value::Null))?;
    println!(
        "REQUEST_PREFIX system={} tools={} total={}",
        system.len(),
        tools.len(),
        prefix_bytes(&params)?
    );
    Ok(())
}

/// The environment block trails the last persisted user block, so the cached prefix a
/// provider matches is the same with or without it.
#[test]
fn the_environment_block_does_not_move_the_cached_prefix() -> TestResult {
    let env = |turn: u32| {
        user(&format!(
            "<environment>\ncwd: /x\nturn: {turn}\n</environment>"
        ))
    };
    let mut first = first_turn();
    first.push(env(1));
    let mut second = second_turn();
    second.push(env(2));
    let a = build_params(&model(), &context(first), &options());
    let b = build_params(&model(), &context(second), &options());
    let strip_env = |params: &Value| -> Result<Value, Box<dyn Error>> {
        let mut out = params.clone();
        let messages = out["messages"].as_array_mut().ok_or("messages")?;
        let last = messages.pop().ok_or("no messages")?;
        assert!(
            last["content"][0].get("cache_control").is_none(),
            "the environment block is never a breakpoint: {last}"
        );
        Ok(out)
    };
    let (a_prefix, b_prefix) = (strip_env(&a)?, strip_env(&b)?);
    assert_prefix_survives_a_turn(&a_prefix, &b_prefix)?;
    let ahead = a_prefix["messages"]
        .as_array()
        .and_then(|m| m.last())
        .and_then(|m| m["content"].as_array())
        .and_then(|blocks| blocks.last())
        .ok_or("no block ahead of the environment")?;
    assert_eq!(ahead["cache_control"]["type"], "ephemeral", "{ahead}");
    Ok(())
}
