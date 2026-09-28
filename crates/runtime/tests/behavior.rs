//! Behavior cassettes (plan §3 micro-tier, D76). Each fixture under
//! tests/fixtures/behavior replays one distilled scenario over the faux
//! provider and prints a BEHAVIOR verdict line that check_behavior.py locks.
//! A red case is data, not a test failure; only a broken harness fails here.

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::collections::VecDeque;
use std::error::Error;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_ai::faux::{faux_assistant_message, faux_text, faux_tool_call};
use yi_context::{Settings, Tokens};
use yi_loop::ExecutionMode;
use yi_runtime::goal::GoalService;
use yi_runtime::permission::PermissionBroker;
use yi_runtime::{AgentSession, PermissionMode, ProviderStream, SessionConfig};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::event::{AgentEvent, ToolResult};
use yi_types::message::{AgentMessage, Content, StopReason, UserContent};
use yi_types::model::{Model, ModelCost};
use yi_types::schedule::DeliveryMode;

/// A malformed cassette, an exhausted stub, or a nondeterministic replay is
/// fatal: the gate must never read a silent null as a verdict.
type Fatal = String;

const EXHAUSTED: &str = "cassette exhausted";

fn need<'a>(value: &'a Value, key: &str) -> Result<&'a Value, Fatal> {
    value.get(key).ok_or_else(|| format!("missing `{key}`"))
}

fn need_str(value: &Value, key: &str) -> Result<String, Fatal> {
    need(value, key)?
        .as_str()
        .map(str::to_owned)
        .ok_or_else(|| format!("`{key}` must be a string"))
}

fn items<'a>(value: &'a Value, key: &str) -> &'a [Value] {
    value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
}

fn flag(value: &Value, key: &str) -> bool {
    value.get(key).and_then(Value::as_bool).unwrap_or(false)
}

fn faux_model(context_window: u64) -> Model {
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
        context_window,
        max_tokens: 16_384,
        compat: None,
        thinking_level_map: None,
        headers: None,
    }
}

/// `goal::run_check` hands its string to `sh -c`, so a cassette may only name
/// checks whose effect is decided here, never an arbitrary command.
fn check_is_allowed(check: &str) -> bool {
    if check == "true" || check == "false" {
        return true;
    }
    if let Some(code) = check.strip_prefix("exit ") {
        return !code.is_empty() && code.bytes().all(|byte| byte.is_ascii_digit());
    }
    match check.strip_prefix("test -f ") {
        Some(path) => is_relative(path),
        None => false,
    }
}

fn is_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains("..")
        && path
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || b"._-/".contains(&byte))
}

struct StubTool {
    name: String,
    results: Mutex<VecDeque<(String, bool)>>,
}

impl Tool for StubTool {
    fn name(&self) -> &str {
        &self.name
    }

    fn description(&self) -> &str {
        "cassette stub: canned results in cassette order"
    }

    fn schema(&self) -> Value {
        json!({"type": "object", "properties": {"command": {"type": "string"}}})
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Exec
    }

    fn execute(&self, _input: Map<String, Value>, _context: &ToolContext) -> ToolOutput {
        let next = self
            .results
            .lock()
            .ok()
            .and_then(|mut queue| queue.pop_front());
        match next {
            Some((text, false)) => text_output(text),
            Some((text, true)) => error_output(text),
            None => error_output(format!("{EXHAUSTED}: {}", self.name)),
        }
    }
}

fn blocks_text(blocks: &[Content]) -> String {
    blocks
        .iter()
        .filter_map(|block| match block {
            Content::Text { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn user_text(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Blocks(blocks) => blocks_text(blocks),
    }
}

fn message_text(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User { content, .. } | AgentMessage::Custom { content, .. } => {
            user_text(content)
        }
        AgentMessage::Assistant { content, .. } | AgentMessage::ToolResult { content, .. } => {
            blocks_text(content)
        }
        AgentMessage::CompactionSummary { summary, .. }
        | AgentMessage::BranchSummary { summary, .. } => summary.clone(),
        _ => String::new(),
    }
}

fn message_kind(message: &AgentMessage) -> String {
    match message {
        AgentMessage::User { .. } => "user".to_owned(),
        AgentMessage::Assistant { stop_reason, .. } => format!("assistant:{stop_reason:?}"),
        AgentMessage::ToolResult {
            tool_name,
            is_error,
            ..
        } => format!("toolResult:{tool_name}:{is_error}"),
        AgentMessage::CompactionSummary { .. } => "compactionSummary".to_owned(),
        other => format!("other:{}", message_text(other).len()),
    }
}

fn result_text(result: &ToolResult) -> String {
    blocks_text(&result.content)
}

fn tool_call_block(call: &Value) -> Result<Content, Fatal> {
    let arguments = call
        .get("arguments")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    Ok(faux_tool_call(
        &need_str(call, "id")?,
        &need_str(call, "name")?,
        arguments,
    ))
}

/// A recorded assistant entry interleaves commentary with parallel calls in one
/// message, so `toolCalls` carries an array beside an optional `text`; the older
/// singular `toolCall` spelling keeps the hand-written cassettes valid.
fn response_message(spec: &Value) -> Result<AgentMessage, Fatal> {
    let mut calls = Vec::new();
    if let Some(call) = spec.get("toolCall") {
        calls.push(tool_call_block(call)?);
    }
    for call in items(spec, "toolCalls") {
        calls.push(tool_call_block(call)?);
    }
    let text = match (spec.get("text"), calls.is_empty()) {
        (Some(_), _) | (None, true) => need_str(spec, "text")?,
        (None, false) => String::new(),
    };
    let mut blocks = Vec::new();
    if !text.is_empty() || calls.is_empty() {
        blocks.push(faux_text(&text));
    }
    let stop = if calls.is_empty() {
        StopReason::Stop
    } else {
        StopReason::ToolUse
    };
    blocks.extend(calls);
    let mut message = faux_assistant_message(blocks, stop);
    if let AgentMessage::Assistant { usage, .. } = &mut message
        && let Some(total) = spec.get("usageTotal").and_then(Value::as_i64)
    {
        let input = spec.get("usageInput").and_then(Value::as_i64).unwrap_or(0);
        usage.input = input;
        usage.output = total.saturating_sub(input);
        usage.total_tokens = total;
    }
    Ok(message)
}

struct Recorded {
    events: Vec<AgentEvent>,
    messages: Vec<AgentMessage>,
    goal_refusal: String,
    goal_status: String,
}

fn materialize(workspace: Option<&Value>, root: &Path) -> Result<(), Fatal> {
    let Some(files) = workspace.and_then(Value::as_object) else {
        return Ok(());
    };
    for (rel, content) in files {
        if !is_relative(rel) {
            return Err(format!("workspace path must be relative: {rel}"));
        }
        let text = content
            .as_str()
            .ok_or_else(|| format!("workspace `{rel}` must be a string"))?;
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        }
        std::fs::write(&path, text).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn cassette_tools(stubs: &[Value]) -> Result<Vec<Arc<dyn Tool>>, Fatal> {
    if stubs.is_empty() {
        return Ok(yi_tools::builtin_tools());
    }
    let mut built: Vec<Arc<dyn Tool>> = Vec::new();
    for stub in stubs {
        let mut queue = VecDeque::new();
        for result in items(stub, "results") {
            queue.push_back((need_str(result, "text")?, flag(result, "isError")));
        }
        built.push(Arc::new(StubTool {
            name: need_str(stub, "name")?,
            results: Mutex::new(queue),
        }));
    }
    Ok(built)
}

fn goal_service(spec: &Value) -> Result<(Arc<GoalService>, yi_session::SharedSession), Fatal> {
    let check = need_str(spec, "check")?;
    if !check_is_allowed(&check) {
        return Err(format!(
            "goal check outside the cassette vocabulary: {check}"
        ));
    }
    let store: yi_session::SharedSession = Arc::new(Mutex::new(
        yi_session::SessionStore::in_memory(yi_session::SessionMetadata {
            id: "behavior".to_owned(),
            created_at: 0,
            parent_session_id: None,
            name: None,
        }),
    ));
    let handle = Arc::clone(&store);
    let service = Arc::new(GoalService::new(
        Arc::new(move || Some(Arc::clone(&handle))),
        Arc::new(|_message, _mode: DeliveryMode| {}),
        std::env::current_dir().map_err(|error| error.to_string())?,
    ));
    service.create(
        &need_str(spec, "objective")?,
        None,
        Some(check),
        spec.get("checkTimeoutMs").and_then(Value::as_u64),
    )?;
    Ok((service, store))
}

async fn drive(cassette: &Value, root: &Path) -> Result<Recorded, Fatal> {
    materialize(cassette.get("workspace"), root)?;

    let provider = Arc::new(ProviderStream::new(None, None));
    for turn in items(cassette, "turns") {
        let mut batch = Vec::new();
        for spec in items(turn, "responses") {
            batch.push(response_message(spec)?);
        }
        provider.queue_faux(batch);
    }

    let compaction = cassette.get("compaction");
    let window = compaction
        .and_then(|spec| spec.get("contextWindow"))
        .and_then(Value::as_u64)
        .unwrap_or(128_000);
    let mut session = AgentSession::new(
        SessionConfig {
            system_prompt: "sys".to_owned(),
            model: faux_model(window),
            thinking_level: None,
            tool_execution: ExecutionMode::Sequential,
        },
        provider,
    );
    if let Some(spec) = compaction {
        session.enable_compaction_with(Settings {
            enabled: true,
            reserve_tokens: Tokens(
                spec.get("reserveTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(1_000),
            ),
            keep_recent_tokens: Tokens(
                spec.get("keepRecentTokens")
                    .and_then(Value::as_u64)
                    .unwrap_or(10),
            ),
        });
    }
    // A broker, as every session has one: the gate then previews each call before it runs
    // it, the path a blind edit once slipped through.
    let broker = Arc::new(PermissionBroker::new(
        PermissionMode::Yolo,
        root.to_path_buf(),
        Vec::new(),
        None,
        tokio::sync::broadcast::channel(8).0,
    ));
    session.use_tools(
        cassette_tools(items(cassette, "stubs"))?,
        root.to_path_buf(),
        Some(broker),
    );

    let goal = match cassette.get("goal") {
        Some(spec) => Some(goal_service(spec)?),
        None => None,
    };

    let mut recorded = Recorded {
        events: Vec::new(),
        messages: Vec::new(),
        goal_refusal: String::new(),
        goal_status: String::new(),
    };
    let mut receiver = session.subscribe();
    for turn in items(cassette, "turns") {
        session
            .prompt(&need_str(turn, "user")?)
            .map_err(|error| error.to_string())?;
        session.wait_idle().await;
        loop {
            match receiver.try_recv() {
                Ok(event) => recorded.events.push(event),
                Err(tokio::sync::broadcast::error::TryRecvError::Lagged(missed)) => {
                    return Err(format!("event channel lagged by {missed}"));
                }
                Err(_) => break,
            }
        }
        if flag(turn, "claimComplete") {
            let (service, _) = goal
                .as_ref()
                .ok_or_else(|| "claimComplete needs a `goal` block".to_owned())?;
            if let Err(refusal) = service.update("complete") {
                recorded.goal_refusal.push_str(&refusal);
            }
        }
    }
    recorded.messages = session.messages();
    if let Some((_, store)) = &goal {
        recorded.goal_status = yi_session::lock_session(store)
            .goal()
            .map(|goal| format!("{:?}", goal.status).to_lowercase())
            .unwrap_or_default();
    }
    for event in &recorded.events {
        if let AgentEvent::ToolExecutionEnd { result, .. } = event
            && result_text(result).contains(EXHAUSTED)
        {
            return Err(format!("{EXHAUSTED}: a cassette stub ran out of results"));
        }
    }
    Ok(recorded)
}

fn evaluate(assertion: &Value, recorded: &Recorded, root: &Path) -> Result<Option<String>, Fatal> {
    let kind = need_str(assertion, "kind")?;
    let call_result = |call: &str| -> Option<(String, bool)> {
        recorded.events.iter().find_map(|event| match event {
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                result,
                is_error,
                ..
            } if tool_call_id == call => Some((result_text(result), *is_error)),
            _ => None,
        })
    };
    let projection = || -> String {
        recorded
            .messages
            .iter()
            .map(message_text)
            .collect::<Vec<_>>()
            .join("\n")
    };
    match kind.as_str() {
        "toolResultContains" | "toolResultIsError" => {
            let call = need_str(assertion, "callId")?;
            let Some((text, is_error)) = call_result(&call) else {
                return Ok(Some(format!("no tool result for call {call}")));
            };
            if kind == "toolResultIsError" {
                let want = flag(assertion, "value");
                return Ok((is_error != want)
                    .then(|| format!("call {call} isError={is_error}, cassette expects {want}")));
            }
            let needle = need_str(assertion, "needle")?;
            Ok((!text.contains(&needle)).then(|| format!("call {call} result lacks {needle:?}")))
        }
        "toolCallCount" => {
            let name = need_str(assertion, "name")?;
            let max = usize::try_from(need(assertion, "max")?.as_u64().unwrap_or(u64::MAX))
                .unwrap_or(usize::MAX);
            let count = recorded
                .events
                .iter()
                .filter(|event| {
                    matches!(event, AgentEvent::ToolExecutionEnd { tool_name, .. } if *tool_name == name)
                })
                .count();
            Ok((count > max).then(|| format!("{name} ran {count} times, cap {max}")))
        }
        "fileEquals" => {
            let rel = need_str(assertion, "path")?;
            if !is_relative(&rel) {
                return Err(format!("fileEquals path must be relative: {rel}"));
            }
            let want = need_str(assertion, "content")?;
            let got = std::fs::read_to_string(root.join(&rel)).unwrap_or_default();
            Ok((got != want).then(|| format!("{rel} holds {got:?}, cassette expects {want:?}")))
        }
        "projectionContains" | "projectionLacks" => {
            let needle = need_str(assertion, "needle")?;
            let present = projection().contains(&needle);
            Ok(match (kind.as_str(), present) {
                ("projectionContains", false) => Some(format!("context lost {needle:?}")),
                ("projectionLacks", true) => Some(format!("context still holds {needle:?}")),
                _ => None,
            })
        }
        "projectionStartsWithSummary" => Ok(matches!(
            recorded.messages.first(),
            Some(AgentMessage::CompactionSummary { .. })
        )
        .then_some(())
        .map_or_else(
            || {
                Some(format!(
                    "context does not start from a compaction summary: {:?}",
                    recorded.messages.first().map(message_kind)
                ))
            },
            |()| None,
        )),
        "finalTextContains" => {
            let needle = need_str(assertion, "needle")?;
            let last = recorded
                .messages
                .iter()
                .rev()
                .find(|message| matches!(message, AgentMessage::Assistant { .. }))
                .map(message_text)
                .unwrap_or_default();
            Ok((!last.contains(&needle)).then(|| format!("final text lacks {needle:?}")))
        }
        "goalRefusalContains" => {
            let needle = need_str(assertion, "needle")?;
            Ok((!recorded.goal_refusal.contains(&needle))
                .then(|| format!("goal refusal lacks {needle:?}: {:?}", recorded.goal_refusal)))
        }
        "goalStatusNot" => {
            let status = need_str(assertion, "status")?;
            Ok((recorded.goal_status == status).then(|| format!("goal status is {status}")))
        }
        other => Err(format!("unknown assertion kind `{other}`")),
    }
}

fn normalize(text: &str, roots: &[String]) -> String {
    let mut out = text.to_owned();
    for root in roots {
        out = out.replace(root.as_str(), "<ws>");
    }
    out.replace('\n', "\\n")
}

fn trace_of(recorded: &Recorded, outcomes: &[Option<String>], roots: &[String]) -> Vec<String> {
    let mut trace: Vec<String> = recorded
        .events
        .iter()
        .filter_map(|event| match event {
            AgentEvent::AgentStart => Some("agent_start".to_owned()),
            AgentEvent::AgentEnd { .. } => Some("agent_end".to_owned()),
            AgentEvent::MessageEnd { message } => {
                Some(format!("message {}", message_kind(message)))
            }
            AgentEvent::ToolExecutionEnd {
                tool_call_id,
                tool_name,
                result,
                is_error,
            } => Some(format!(
                "tool {tool_name} {tool_call_id} error={is_error} {}",
                normalize(&result_text(result), roots)
            )),
            _ => None,
        })
        .collect();
    for message in &recorded.messages {
        trace.push(format!(
            "projection {} {}",
            message_kind(message),
            normalize(&message_text(message), roots)
        ));
    }
    trace.push(format!(
        "goal {} {}",
        recorded.goal_status,
        normalize(&recorded.goal_refusal, roots)
    ));
    for (index, outcome) in outcomes.iter().enumerate() {
        trace.push(match outcome {
            Some(failure) => format!("assert {index} FAIL {}", normalize(failure, roots)),
            None => format!("assert {index} ok"),
        });
    }
    trace
}

async fn replay(cassette: &Value, id: &str, run: u32) -> Result<(Vec<String>, Vec<String>), Fatal> {
    let root =
        Scratch::new(&format!("yi-behavior-{id}-{run}")).map_err(|error| error.to_string())?;
    let mut roots = vec![root.to_string_lossy().into_owned()];
    if let Ok(canonical) = root.canonicalize() {
        roots.insert(0, canonical.to_string_lossy().into_owned());
    }

    let recorded = drive(cassette, &root).await?;
    let mut outcomes = Vec::new();
    for assertion in items(cassette, "assertions") {
        outcomes.push(evaluate(assertion, &recorded, &root)?);
    }
    let trace = trace_of(&recorded, &outcomes, &roots);
    Ok((trace, outcomes.into_iter().flatten().collect::<Vec<_>>()))
}

fn one_line(text: &str) -> String {
    text.chars()
        .map(|character| {
            if character.is_control() {
                ' '
            } else {
                character
            }
        })
        .take(160)
        .collect()
}

/// One BEHAVIOR line per cassette, in filename order. The test fails only when
/// the instrument itself is broken — a red case is the baseline's business.
#[tokio::test]
async fn behavior_cassettes_replay_deterministically() -> Result<(), Box<dyn Error>> {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/behavior");
    let mut paths: Vec<PathBuf> = std::fs::read_dir(&dir)?
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    if paths.is_empty() {
        return Err(format!("no cassettes under {}", dir.display()).into());
    }
    for path in paths {
        let stem = path
            .file_stem()
            .map(|stem| stem.to_string_lossy().into_owned())
            .unwrap_or_default();
        let cassette: Value = serde_json::from_str(&std::fs::read_to_string(&path)?)?;
        let id = need_str(&cassette, "id")?;
        if id != stem {
            return Err(format!("cassette id {id} does not match filename {stem}").into());
        }
        need_str(&cassette, "description")?;
        need_str(need(&cassette, "provenance")?, "kind")?;
        let (first_trace, failures) = replay(&cassette, &id, 0).await?;
        let (second_trace, _) = replay(&cassette, &id, 1).await?;
        if first_trace != second_trace {
            let diff = first_trace
                .iter()
                .zip(second_trace.iter())
                .find(|(one, two)| one != two)
                .map(|(one, two)| format!("{one} != {two}"))
                .unwrap_or_else(|| "trace lengths differ".to_owned());
            return Err(format!("case {id} replays nondeterministically: {diff}").into());
        }
        let state = if failures.is_empty() { "pass" } else { "fail" };
        let detail = failures
            .first()
            .map_or_else(|| "-".to_owned(), |first| one_line(first));
        println!("BEHAVIOR case={id} state={state} detail={detail}");
    }
    Ok(())
}
