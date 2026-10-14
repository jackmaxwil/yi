use std::sync::Arc;

use serde_json::{Map, Value};
use yi_permission::{ReviewedAsk, UserVerdict};
use yi_types::message::AgentMessage;
use yi_types::model::Model;

use crate::provider::ProviderStream;
use crate::session::{AgentSession, SessionConfig};

pub const AUTO_REVIEW_PROMPT: &str = include_str!("prompts/auto_review.md");

/// Incident: the reviewer sits in front of every unprovable command in auto mode, so a
/// stalled provider would stall the agent. Past the cap the answer is a denial, not a wait.
pub const REVIEW_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// One appended sentence when the role is named (§8): a model that is not
/// told a denial is answerable just retries the denied call.
pub fn review_fragment() -> &'static str {
    "An auto reviewer screens calls this mode cannot prove safe. A refusal names a request number; call ask_user with that number to put the call to the user, and never re-issue the same call hoping for a different answer."
}

pub struct ReviewRequest {
    pub tool: String,
    pub display: String,
    pub command: Option<String>,
    pub cwd: String,
    pub targets: Vec<String>,
    pub patch: Option<String>,
    pub reason: String,
}

impl ReviewRequest {
    /// The reviewed text is fenced and labeled so the reviewer can tell the
    /// action from the instructions about judging it.
    fn render(&self) -> String {
        let mut out = format!("Working directory: {}\n", self.cwd);
        out.push_str(&format!("Deterministic policy said: {}\n", self.reason));
        out.push_str("\n<action trust=\"untrusted\">\n");
        out.push_str(&format!("tool: {}\n", self.tool));
        out.push_str(&format!("summary: {}\n", self.display));
        if let Some(command) = &self.command {
            out.push_str(&format!("command: {command}\n"));
        }
        for target in &self.targets {
            out.push_str(&format!("touches: {target}\n"));
        }
        if let Some(patch) = &self.patch {
            out.push_str(&format!("patch:\n{patch}\n"));
        }
        out.push_str("</action>\n");
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewOutcome {
    Allow,
    Deny { reason: String },
}

/// Invariant: every path that is not a well-formed `allow` is a denial —
/// malformed output, an empty answer, a provider error and the timeout alike.
pub fn parse_outcome(answer: &str) -> ReviewOutcome {
    let line = answer
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or_default();
    if line.eq_ignore_ascii_case("allow") {
        return ReviewOutcome::Allow;
    }
    let rest = line
        .strip_prefix("deny")
        .or_else(|| line.strip_prefix("Deny"))
        .map(str::trim)
        .filter(|rest| !rest.is_empty());
    ReviewOutcome::Deny {
        reason: match rest {
            Some(rest) => rest.to_owned(),
            None => "the reviewer's answer was not `allow` or `deny <reason>`".to_owned(),
        },
    }
}

/// Its own session, one prompt per review, no tools: the reviewer answers in
/// one line and has nothing to act with.
pub struct Reviewer {
    provider: Arc<ProviderStream>,
    model: Model,
}

impl Reviewer {
    pub fn new(provider: Arc<ProviderStream>, model: Model) -> Self {
        Self { provider, model }
    }

    pub async fn review(&self, request: &ReviewRequest) -> ReviewOutcome {
        let mut session = AgentSession::new(
            SessionConfig {
                system_prompt: AUTO_REVIEW_PROMPT.to_owned(),
                model: self.model.clone(),
                thinking_level: None,
                tool_execution: yi_loop::ExecutionMode::Sequential,
            },
            Arc::clone(&self.provider),
        );
        session.set_tools(Vec::new());
        session.set_reuse(yi_types::model::Reuse::OneShot);
        if session.prompt(&request.render()).is_err() {
            return ReviewOutcome::Deny {
                reason: "the reviewer session refused the prompt".to_owned(),
            };
        }
        session.wait_idle().await;
        let answer = session
            .messages()
            .iter()
            .rev()
            .find_map(|message| match message {
                AgentMessage::Assistant { content, .. } => {
                    Some(crate::advisor::digest::assistant_text(content))
                }
                _ => None,
            })
            .unwrap_or_default();
        parse_outcome(&answer)
    }
}

/// `request` takes the broker the reviewer was wired into: a denial that names a request the
/// model cannot answer is worse than no reviewer at all.
pub struct AskUserTool {
    broker: Option<Arc<crate::permission::PermissionBroker>>,
    parent: Option<crate::mailbox::ParentLink>,
    unattended: bool,
}

impl AskUserTool {
    pub fn new(broker: Option<Arc<crate::permission::PermissionBroker>>) -> Self {
        Self {
            broker,
            parent: None,
            unattended: false,
        }
    }

    pub fn asking(mut self, parent: Option<crate::mailbox::ParentLink>) -> Self {
        self.parent = parent;
        self
    }

    pub fn unattended(mut self, unattended: bool) -> Self {
        self.unattended = unattended;
        self
    }
}

/// One `ask_user` option as the result text and the question card both read it.
pub struct AskOption<'a> {
    pub label: &'a str,
    pub description: Option<&'a str>,
    pub preview: Option<&'a str>,
}

/// Invariant: the one reader of `options`, so the number the model is shown and the key the
/// person presses name the same option; an entry with no label is dropped from both.
pub fn ask_options(options: Option<&Value>) -> Vec<AskOption<'_>> {
    fn text<'a>(option: &'a Value, key: &str) -> Option<&'a str> {
        let text = option.get(key)?.as_str()?.trim();
        (!text.is_empty()).then_some(text)
    }
    let options = options.and_then(Value::as_array).into_iter().flatten();
    options
        .filter_map(|option| {
            let label = match option {
                Value::String(label) => Some(label.trim()).filter(|label| !label.is_empty()),
                _ => text(option, "label"),
            }?;
            let preview = option.get("preview").and_then(Value::as_str);
            Some(AskOption {
                label,
                description: text(option, "description"),
                preview: preview
                    .filter(|preview| !preview.trim().is_empty())
                    .map(str::trim_end),
            })
        })
        .collect()
}

fn question_text(input: &Map<String, Value>) -> Option<String> {
    let question = input.get("question").and_then(Value::as_str)?.trim();
    if question.is_empty() {
        return None;
    }
    let mut text = question.to_owned();
    for (index, option) in ask_options(input.get("options")).iter().enumerate() {
        let number = index.saturating_add(1);
        match option.description {
            Some(description) => {
                text.push_str(&format!("\n  {number}. {}: {description}", option.label));
            }
            None => text.push_str(&format!("\n  {number}. {}", option.label)),
        }
    }
    if let Some(default) = input.get("default").and_then(Value::as_str) {
        text.push_str(&format!("\nDefault if unanswered: {default}"));
    }
    Some(text)
}

const SCHEMA: &str = r#"{"type":"object","properties":{"question":{"type":"string","description":"What only the user can decide, in one sentence"},"options":{"type":"array","items":{"type":"object","properties":{"label":{"type":"string"},"description":{"type":"string","description":"One line: what it costs or buys"},"preview":{"type":"string","description":"A mock, snippet or prose, five lines"}},"required":["label"]},"description":"Two to four choices, the recommended one first"},"default":{"type":"string","description":"The option you proceed on if the user does not answer"},"request":{"type":"integer","description":"The request number quoted in an auto-review denial."}}}"#;

pub const ASK_USER_GUIDE: &str = "Ask the user something only they can decide, and end the turn on it. Pass `question` with two to four `options` and a `default` when one exists; never write a multiple-choice question as prose. Block the todo it waits on first. With auto-review on, `request` (the number a denial quoted) puts that denial to the user instead; a denial you do not escalate stays denied.\n\nIn design talk (brainstorm, change plan, architecture, targets, a choice between looks) ask more: read first, then put up to four questions in one turn, one call each, in plain words. Each option is {label, description}: a few words, then one line on what it costs or buys. Option 1 is your recommendation. When options differ in how they look or run, add a `preview` (a mock, snippet or prose; five lines show). A skip means go with option 1 and say so.";

impl yi_tools::Tool for AskUserTool {
    fn name(&self) -> &str {
        "ask_user"
    }

    fn description(&self) -> &str {
        if self.parent.is_some() {
            return "Ask your parent something only it can decide; the call waits for its answer and returns it. Pass `question` with two to four `options` and a `default` when one exists; never write a multiple-choice question as prose.";
        }
        "Ask the user a question only they can decide, with two to four `options` rather than as prose; ends the turn. Guide: read(\"yi://tools/ask_user\")."
    }

    fn schema(&self) -> Value {
        serde_json::from_str(SCHEMA).unwrap_or_default()
    }

    fn kind(&self) -> yi_tools::ToolKind {
        yi_tools::ToolKind::Read
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        match (
            input.get("request").and_then(Value::as_u64),
            question_text(input),
        ) {
            (Some(_), _) if self.broker.is_some() => Ok(()),
            (Some(_), _) => {
                Err("ask_user `request` needs auto-review on; ask a `question` instead".to_owned())
            }
            (None, Some(_)) => Ok(()),
            (None, None) => Err(
                "ask_user needs a `question` (or `request`, the number a denial quoted)".to_owned(),
            ),
        }
    }

    fn execute(
        &self,
        input: Map<String, Value>,
        context: &yi_tools::ToolContext,
    ) -> yi_tools::ToolOutput {
        if let (Some(request), Some(broker)) =
            (input.get("request").and_then(Value::as_u64), &self.broker)
        {
            return yi_tools::text_output(broker.resolve_request(request, &context.call_id));
        }
        let Some(text) = question_text(&input) else {
            return yi_tools::error_output(
                "ask_user needs a `question` (or `request`, the number a denial quoted)",
            );
        };
        if let Some(parent) = &self.parent {
            return match parent.ask(&text, &context.cancelled) {
                Ok(answer) => {
                    let mut output =
                        yi_tools::text_output(format!("Your parent answered: {answer}"));
                    let value = yi_types::tool_value::AnswerValue { answer };
                    output.value = (context.program)
                        .then(|| serde_json::to_value(value).unwrap_or(Value::Null));
                    output
                }
                Err(refusal) => yi_tools::error_output(refusal),
            };
        }
        if self.unattended {
            let options = ask_options(input.get("options"));
            let default = input.get("default").and_then(Value::as_str);
            let taken = default.or_else(|| options.first().map(|option| option.label));
            return yi_tools::text_output(format!(
                "Nobody is attached to answer: {text}\nGo on with {}, and name that assumption in the report.",
                taken.unwrap_or("your recommendation")
            ));
        }
        let mut output = yi_tools::text_output(format!(
            "Question for the user: {text}\nThe turn ends here; the user's next message is the answer."
        ));
        output.result.terminate = Some(true);
        output
    }
}

/// What a resolved request tells the model, so the next step is unambiguous
/// whichever way the user answered.
pub fn resolution_text(display: &str, verdict: UserVerdict) -> String {
    match verdict {
        UserVerdict::Approved => format!(
            "The user approved this call once: {display}. Re-issue it exactly as it was; any change makes it a different call that has to be asked about again."
        ),
        UserVerdict::Denied => format!(
            "The user denied this call: {display}. It stays denied — take another approach rather than re-issuing it."
        ),
    }
}

/// Model-role-gated the way the advisor is (D28/D50): with the role unnamed no reviewer is
/// constructed and auto mode stays deterministic; `ask_user` is registered either way.
pub fn wire(
    session: &AgentSession,
    wiring: &crate::wiring::RuntimeWiring,
    tools: &mut Vec<Arc<dyn yi_tools::Tool>>,
) {
    wire_role(
        session,
        (wiring.auto_review.clone(), wiring.broker.clone()),
        &wiring.provider,
        tools,
        wiring.parent_link.clone(),
    );
}

pub fn wire_role(
    session: &AgentSession,
    (role, broker): (
        Option<Model>,
        Option<Arc<crate::permission::PermissionBroker>>,
    ),
    provider: &Arc<ProviderStream>,
    tools: &mut Vec<Arc<dyn yi_tools::Tool>>,
    parent: Option<crate::mailbox::ParentLink>,
) {
    // Invariant: a question on a root nobody attends would end the run unanswered, so it
    // returns the default to act on instead; a child's parent always answers.
    let unattended = parent.is_none() && broker.as_ref().is_some_and(|broker| broker.unattended());
    let (Some(model), Some(broker)) = (role, broker) else {
        let ask = AskUserTool::new(None).asking(parent).unattended(unattended);
        tools.push(Arc::new(ask));
        return;
    };
    broker.set_reviewer(Arc::new(Reviewer::new(Arc::clone(provider), model)));
    // P3: tools freeze after SessionStart, so registration is here or nowhere.
    tools.push(Arc::new(
        AskUserTool::new(Some(Arc::clone(&broker)))
            .asking(parent)
            .unattended(unattended),
    ));
    if let Some(host) = session.extensions()
        && let Ok(mut host) = host.lock()
    {
        host.attach(
            crate::ext::Slot::new(crate::ext::Rank::Mode, "auto-review"),
            review_fragment().to_owned(),
        );
    }
}

pub fn ask_text(ask: &ReviewedAsk) -> crate::permission::PermissionAsk<'_> {
    crate::permission::PermissionAsk {
        title: &ask.title,
        description: &ask.description,
        patch: ask.patch.as_deref(),
        changes: &ask.targets,
        grants: &ask.grants,
        tool_call_id: None,
    }
}
