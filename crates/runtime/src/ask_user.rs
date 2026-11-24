use std::sync::Arc;

use serde_json::{Map, Value};

#[derive(Default)]
pub struct AskUserTool {
    parent: Option<crate::mailbox::ParentLink>,
    unattended: bool,
}

impl AskUserTool {
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

const SCHEMA: &str = r#"{"type":"object","properties":{"question":{"type":"string","description":"What only the user can decide, in one sentence"},"options":{"type":"array","items":{"type":"object","properties":{"label":{"type":"string"},"description":{"type":"string","description":"One line: what it costs or buys"},"preview":{"type":"string","description":"A mock, snippet or prose, five lines"}},"required":["label"]},"description":"Two to four choices, the recommended one first"},"default":{"type":"string","description":"The option you proceed on if the user does not answer"}}}"#;

pub const ASK_USER_GUIDE: &str = "Ask the user something only they can decide, and end the turn on it. Pass `question` with two to four `options` and a `default` when one exists; never write a multiple-choice question as prose. Block the todo it waits on first.\n\nIn design talk (brainstorm, change plan, architecture, targets, a choice between looks) ask more: read first, then put up to four questions in one turn, one call each, in plain words. Each option is {label, description}: a few words, then one line on what it costs or buys. Option 1 is your recommendation. When options differ in how they look or run, add a `preview` (a mock, snippet or prose; five lines show). A skip means go with option 1 and say so.";

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
        question_text(input)
            .map(|_| ())
            .ok_or_else(|| "ask_user needs a `question`".to_owned())
    }

    fn execute(
        &self,
        input: Map<String, Value>,
        context: &yi_tools::ToolContext,
    ) -> yi_tools::ToolOutput {
        let Some(text) = question_text(&input) else {
            return yi_tools::error_output("ask_user needs a `question`");
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

pub fn wire(wiring: &crate::wiring::RuntimeWiring, tools: &mut Vec<Arc<dyn yi_tools::Tool>>) {
    // Invariant: a question on a root nobody attends would end the run unanswered, so it
    // returns the default to act on instead; a child's parent always answers.
    let parent = wiring.parent_link.clone();
    let unattended = parent.is_none()
        && wiring
            .broker
            .as_ref()
            .is_some_and(|broker| broker.unattended());
    let ask = AskUserTool::default().asking(parent).unattended(unattended);
    tools.push(Arc::new(ask));
}
