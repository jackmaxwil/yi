mod config;
mod exec;
mod shape;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use serde_json::{Map, Value};
use yi_tools::CancelFlag;
use yi_types::event::ToolResult;
use yi_types::message::Content;

pub use config::{Hooks, PROJECT_FILES, carries_hooks};

pub const RESPONSE_CAP: usize = 64 * 1024;

/// Claude Code's own cap on a hook's `additionalContext` and plain text, in characters.
pub const HOOK_TEXT_CAP: usize = 10_000;

/// What a cut hook text still shows when a file keeps every character, as Claude Code's does.
const PREVIEW_CHARS: usize = 2_000;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    PreToolUse,
    PostToolUse,
    PostToolUseFailure,
}

impl Event {
    fn as_str(self) -> &'static str {
        match self {
            Self::PreToolUse => "PreToolUse",
            Self::PostToolUse => "PostToolUse",
            Self::PostToolUseFailure => "PostToolUseFailure",
        }
    }

    fn parse(name: &str) -> Option<Self> {
        [
            Self::PreToolUse,
            Self::PostToolUse,
            Self::PostToolUseFailure,
        ]
        .into_iter()
        .find(|event| event.as_str() == name)
    }
}

/// Invariant: handlers combine by rank, deny over ask over allow; a silent one changes nothing.
#[derive(Clone, Default, PartialEq, Eq, Debug)]
pub enum Verdict {
    #[default]
    Silent,
    Allow,
    Ask(String),
    Deny(String),
}

impl Verdict {
    fn rank(&self) -> u8 {
        match self {
            Self::Silent => 0,
            Self::Allow => 1,
            Self::Ask(_) => 2,
            Self::Deny(_) => 3,
        }
    }
}

#[derive(Default)]
pub struct Outcome {
    pub verdict: Verdict,
    /// `continue: false`: the reason the run stops after this call.
    pub stop: Option<String>,
    pub rows: Vec<String>,
    pub updated_input: Option<Map<String, Value>>,
    pub updated_output: Option<Value>,
}

#[derive(Clone, Default)]
pub struct Call {
    pub tool: String,
    pub args: Map<String, Value>,
    pub tool_use_id: String,
    pub cwd: PathBuf,
    pub session_id: Option<String>,
    pub transcript_path: Option<PathBuf>,
    pub permission_mode: Option<&'static str>,
    pub agent_id: Option<String>,
    pub response: Option<Value>,
    pub error: Option<String>,
    pub interrupted: bool,
    /// What an edit changes in each file, prepared before it runs; Edit hooks read the span.
    pub edits: Vec<yi_tools::FileChange>,
    pub spills: Option<PathBuf>,
}

pub fn mode_name(mode: yi_permission::PermissionMode) -> &'static str {
    match mode {
        yi_permission::PermissionMode::Ask => "default",
        yi_permission::PermissionMode::Auto => "acceptEdits",
        yi_permission::PermissionMode::Yolo => "bypassPermissions",
    }
}

/// Past [`HOOK_TEXT_CAP`] a text is kept whole in a file under `spills` and a preview shown.
fn capped(text: &str, what: &str, spills: Option<&Path>) -> String {
    let total = text.chars().count();
    if total <= HOOK_TEXT_CAP {
        return text.to_owned();
    }
    let spill = yi_tools::CellSpill::new(spills);
    (spill.sink())("stdout", text);
    let (kept, rest) = match spill.note(0).as_deref().and_then(yi_tools::spill_path) {
        Some((_, path)) => (PREVIEW_CHARS, format!("every character: read {path}")),
        None => (
            HOOK_TEXT_CAP,
            "no file kept the rest: run the hook's command by hand to read it all".to_owned(),
        ),
    };
    let head: String = text.chars().take(kept).collect();
    format!(
        "{head}\n[… kept {kept} of {total} characters of the hook's {what} (HOOK_TEXT_CAP {HOOK_TEXT_CAP}); {rest}]"
    )
}

fn raise(outcome: &mut Outcome, verdict: Verdict) {
    if verdict.rank() > outcome.verdict.rank() {
        outcome.verdict = verdict;
    }
}

pub fn refusal(outcome: &Outcome) -> Option<String> {
    refusal_unless_answerable(outcome, false)
}

pub(crate) fn refusal_unless_answerable(outcome: &Outcome, can_answer: bool) -> Option<String> {
    if let Some(reason) = &outcome.stop {
        return Some(format!("the run stops: {reason}"));
    }
    match &outcome.verdict {
        Verdict::Deny(reason) => Some(reason.clone()),
        Verdict::Ask(_) if can_answer => None,
        Verdict::Ask(why) => Some(format!(
            "asks to confirm this call ({why}), and no one can answer here"
        )),
        Verdict::Allow | Verdict::Silent => None,
    }
}

fn pre_verdict(decision: Option<&str>, reason: &dyn Fn(&str) -> String) -> Result<Verdict, String> {
    Ok(match decision {
        None => Verdict::Silent,
        Some("allow" | "approve") => Verdict::Allow,
        Some("ask") => Verdict::Ask(reason("a Claude Code PreToolUse hook asks")),
        Some("deny" | "block") => Verdict::Deny(reason("denied by a Claude Code PreToolUse hook")),
        Some(other) => return Err(other.to_owned()),
    })
}

const DEFERRED: &str = "[Claude Code PreToolUse hook deferred this call; Yi ignores a defer, as Claude Code does outside a single-call -p run, so the call goes on through the permission gate]";

fn read_fields(event: Event, value: &Value, decides: bool, call: &Call, outcome: &mut Outcome) {
    let spills = call.spills.as_deref();
    let text = |value: Option<&Value>| value.and_then(Value::as_str).map(str::to_owned);
    if value.get("continue") == Some(&Value::Bool(false)) {
        let reason = text(value.get("stopReason"));
        let reason = reason.unwrap_or_else(|| "a Claude Code hook stopped the run".to_owned());
        outcome.stop = Some(capped(&reason, "stopReason", spills));
    }
    let specific = value.get("hookSpecificOutput");
    let field = |key: &str| specific.and_then(|output| output.get(key));
    let decision = (text(field("permissionDecision")).or_else(|| text(value.get("decision"))))
        .filter(|_| event == Event::PreToolUse);
    if decision.as_deref() == Some("defer") {
        outcome.rows.extend(decides.then(|| DEFERRED.to_owned()));
        return;
    }
    if let Some(context) = text(field("additionalContext")) {
        let context = capped(&context, "additionalContext", spills);
        outcome
            .rows
            .push(format!("[Claude Code hook context: {context}]"));
    }
    let said = text(field("permissionDecisionReason")).or_else(|| text(value.get("reason")));
    let reason = |default: &str| capped(said.as_deref().unwrap_or(default), "reason", spills);
    match event {
        Event::PreToolUse => {
            match pre_verdict(decision.as_deref().filter(|_| decides), &reason) {
                Ok(verdict) => raise(outcome, verdict),
                Err(other) => {
                    let other = capped(&other, "permissionDecision", spills);
                    outcome.rows.push(format!(
                        "[Claude Code PreToolUse hook error: unknown permissionDecision {other:?}]"
                    ));
                }
            }
            if let Some(Value::Object(updated)) = field("updatedInput") {
                match &outcome.updated_input {
                    Some(seen) if seen != updated => {
                        let why = "two Claude Code hooks returned different updatedInput";
                        raise(outcome, Verdict::Deny(why.to_owned()));
                    }
                    _ => outcome.updated_input = Some(updated.clone()),
                }
            }
        }
        Event::PostToolUse | Event::PostToolUseFailure => {
            if decides && text(value.get("decision")).as_deref() == Some("block") {
                let reason = reason("blocked by a Claude Code hook");
                outcome
                    .rows
                    .push(format!("[Claude Code hook feedback: {reason}]"));
            }
            let replaced = field("updatedToolOutput").or_else(|| field("updatedMCPToolOutput"));
            if let Some(replaced) = replaced.filter(|_| event == Event::PostToolUse) {
                outcome.updated_output = Some(replaced.clone());
            }
        }
    }
}

fn merge(event: Event, said: exec::Said, call: &Call, outcome: &mut Outcome) {
    let spills = call.spills.as_deref();
    match said {
        exec::Said::Quiet => {}
        exec::Said::Notice(text) => {
            let text = capped(&text, "error", spills);
            (outcome.rows).push(format!(
                "[Claude Code {} hook error: {text}]",
                event.as_str()
            ));
        }
        exec::Said::Json(value) => read_fields(event, &value, true, call, outcome),
        exec::Said::Blocked { reason, json, cut } => {
            // Exit 2 blocks whatever the JSON says, but a blocking decision's reason wins.
            let mut decided = Outcome::default();
            if let Some(json) = &json {
                read_fields(event, json, true, call, &mut decided);
                read_fields(event, json, false, call, outcome);
            }
            let blocked =
                (json.as_ref()).filter(|json| json.get("decision") == Some(&Value::from("block")));
            let said = blocked
                .and_then(|json| json.get("reason"))
                .and_then(Value::as_str);
            let reason = match (event, decided.verdict, said) {
                (Event::PreToolUse, Verdict::Deny(reason), _) => reason,
                (Event::PostToolUse | Event::PostToolUseFailure, _, Some(said)) => {
                    capped(said, "reason", spills)
                }
                _ => capped(&reason, "stderr", spills),
            };
            let reason = match cut {
                Some(cut) => format!("{reason}\n{cut}"),
                None => reason,
            };
            match event {
                Event::PreToolUse => raise(outcome, Verdict::Deny(reason)),
                Event::PostToolUse | Event::PostToolUseFailure => {
                    outcome
                        .rows
                        .push(format!("[Claude Code hook feedback: {reason}]"));
                }
            }
        }
    }
}

impl Hooks {
    /// Every matching handler runs at once, for every file of the call, an identical one once.
    pub fn run(&self, event: Event, call: &Call, cancel: &CancelFlag) -> Outcome {
        let name = shape::claude_name(&call.tool, &call.args);
        let mut matched: Vec<&config::Handler> = Vec::new();
        for handler in (self.handlers.iter())
            .filter(|handler| handler.event == event && handler.matcher.matches(&name))
        {
            if !matched.iter().any(|seen| seen.key() == handler.key()) {
                matched.push(handler);
            }
        }
        let mut outcome = Outcome::default();
        if matched.is_empty() {
            return outcome;
        }
        let places = (call.cwd.as_path(), self.project.as_path());
        let inputs = shape::tool_inputs(call);
        let mut runs = Vec::new();
        for (input, notice) in &inputs {
            outcome.rows.extend(notice.clone());
            let stdin = shape::stdin(event, call, &name, input.clone());
            let admitted = (matched.iter()).filter(|handler| {
                (handler.condition.as_ref()).is_none_or(|rule| rule.admits(&name, input, &call.cwd))
            });
            for handler in admitted {
                match handler.detached {
                    true => exec::detach(handler, stdin.clone(), places.0, places.1),
                    false => runs.push((*handler, stdin.clone())),
                }
            }
        }
        let said: Vec<exec::Said> = std::thread::scope(|scope| {
            let running: Vec<_> = (runs.iter())
                .map(|(handler, stdin)| scope.spawn(|| exec::run(handler, stdin, places, cancel)))
                .collect();
            (running.into_iter())
                .map(|thread| {
                    let panicked = || exec::Said::Notice("its thread panicked".to_owned());
                    thread.join().unwrap_or_else(|_| panicked())
                })
                .collect()
        });
        for said in said {
            merge(event, said, call, &mut outcome);
        }
        // Each row is capped alone; a patch's files are capped together, as one Claude edit's is.
        let joined = outcome.rows.join("\n");
        if inputs.len() > 1 && joined.chars().count() > HOOK_TEXT_CAP {
            let what = format!("rows across the {} files of this call", inputs.len());
            outcome.rows = vec![capped(&joined, &what, call.spills.as_deref())];
        }
        outcome
    }

    fn matches(&self, event: Event, call: &Call) -> bool {
        let name = shape::claude_name(&call.tool, &call.args);
        (self.handlers.iter())
            .any(|handler| handler.event == event && handler.matcher.matches(&name))
    }

    pub(crate) fn needs_changes(&self, event: Event, call: &Call) -> bool {
        call.tool == "edit" && self.matches(event, call)
    }
}

/// A shell command the runtime runs for the model off the tool seam (a goal or plan check, a
/// schedule's `exec://`) passes the Bash PreToolUse hooks first; Err is the refusal.
pub(crate) fn guard_shell(hooks: Option<&Hooks>, command: &str, cwd: &Path) -> Result<(), String> {
    let Some(hooks) = hooks else {
        return Ok(());
    };
    let call = Call {
        tool: "bash".to_owned(),
        args: Map::from_iter([("command".to_owned(), Value::from(command))]),
        cwd: cwd.to_path_buf(),
        ..Call::default()
    };
    let never: CancelFlag = Arc::new(|| false);
    let outcome = hooks.run(Event::PreToolUse, &call, &never);
    refusal(&outcome).map_or(Ok(()), |why| {
        Err(format!(
            "Claude Code PreToolUse hook refused `{command}`: {why}"
        ))
    })
}

#[derive(Clone)]
pub(crate) struct Attached {
    hooks: Arc<Hooks>,
    announced: Arc<AtomicBool>,
}

impl Attached {
    pub(crate) fn of(hooks: Option<Arc<Hooks>>) -> Option<Self> {
        let hooks = hooks?;
        let announced = Arc::new(AtomicBool::new(false));
        Some(Self { hooks, announced })
    }

    pub(crate) fn needs_changes(&self, event: Event, call: &Call) -> bool {
        self.hooks.needs_changes(event, call)
    }

    pub(crate) async fn after(
        &self,
        call: Call,
        result: &mut ToolResult,
        failed: bool,
        pre: &[String],
        cancel: CancelFlag,
    ) {
        let (tool, spills) = (call.tool.clone(), call.spills.clone());
        let (event, call) = shape::after(call, result, failed);
        let post = self.run(event, call, cancel).await;
        if let Some(value) = &post.updated_output {
            match shape::replaced_output(&tool, value) {
                Some(text) => {
                    let text = capped(&text, "updatedToolOutput", spills.as_deref());
                    let text_signature = None;
                    result.content = vec![Content::Text {
                        text,
                        text_signature,
                    }];
                }
                None => crate::affordance::append(
                    result,
                    "[Claude Code PostToolUse hook error: its updatedToolOutput is not this tool's output shape, so the output stands]",
                ),
            }
        }
        for row in pre.iter().chain(&post.rows) {
            crate::affordance::append(result, row);
        }
        if let Some(reason) = post.stop {
            crate::affordance::append(
                result,
                &format!("[Claude Code hook stopped the run: {reason}]"),
            );
            result.terminate = Some(true);
        }
    }

    /// The configuration's notices ride the session's first pre-hook outcome, and only it.
    pub(crate) async fn run(&self, event: Event, call: Call, cancel: CancelFlag) -> Outcome {
        let hooks = Arc::clone(&self.hooks);
        let ran = tokio::task::spawn_blocking(move || hooks.run(event, &call, &cancel)).await;
        let mut outcome = ran.unwrap_or_else(|error| Outcome {
            rows: vec![format!(
                "[Claude Code {} hooks failed: {error}]",
                event.as_str()
            )],
            ..Outcome::default()
        });
        if event == Event::PreToolUse && !self.announced.swap(true, Ordering::SeqCst) {
            let rows = std::mem::take(&mut outcome.rows);
            outcome.rows = self.hooks.notices.iter().cloned().chain(rows).collect();
        }
        outcome
    }
}

pub(crate) use shape::yi_args as updated_args;
