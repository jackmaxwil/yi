use std::path::PathBuf;
use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};

use crate::session::AnswerHandle;

const HELD: &str = "The answer is held, not shown. Fix what this names, patch any draft below with `edit`, then call `answer` with only `summary`, or with a new `text`.";
pub(crate) const GUIDE: &str = "# answer\n\nThe last call of a turn: `text` is the whole answer, `summary` one line on what changed or was found.\n\nIt is checked before anyone sees it: open todos, numbers no tool printed, an open plan step. A refusal holds the draft and prints it as `[path#TAG]` rows: do the missing work, patch those rows with `edit`, and call `answer` with only `summary`. Nothing after an accepted answer reaches the user.";

pub(crate) struct AnswerTool {
    handle: AnswerHandle,
    write: Option<Arc<dyn Tool>>,
}

pub(crate) fn tool(handle: AnswerHandle, tools: &[Arc<dyn Tool>]) -> Arc<dyn Tool> {
    let write = (tools.iter()).find(|tool| tool.name() == "write").cloned();
    Arc::new(AnswerTool { handle, write })
}

impl AnswerTool {
    /// The draft written through `write`, so its tag anchors an `edit`, and shown as `read`
    /// shows it; None with no spill dir or when a wall refuses the write.
    fn hold_file(&self, draft: &str, context: &ToolContext) -> Option<(PathBuf, String)> {
        let stem = (context.transcript.as_ref()).and_then(|path| path.file_stem());
        let stem = stem.map_or_else(|| "draft".into(), |stem| stem.to_string_lossy());
        let path = context.recovery_dir.as_ref()?.join("answers");
        std::fs::create_dir_all(&path).ok()?;
        let path = path.join(format!("{stem}.md"));
        let input = json!({"path": path.to_string_lossy(), "content": draft});
        let written = (self.write.as_ref()?).execute(input.as_object()?.clone(), context);
        let header = yi_types::message::join_text(&written.result.content, "\n");
        let header = header.lines().next().filter(|_| !written.is_error)?;
        let rows = yi_tools::hashline::format::format_numbered_lines(draft, 1);
        Some((path, format!("{header}\n{rows}")))
    }
}

impl Tool for AnswerTool {
    fn name(&self) -> &str {
        "answer"
    }

    fn description(&self) -> &str {
        "End the turn, called alone and last: `text` is the final answer, `summary` one line. Checked before it is shown; a refusal holds the draft for `edit`. Guide: read(\"yi://tools/answer\")."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "summary": {"type": "string"},
                "text": {"type": "string", "description": "markdown; omit to submit the held draft"}
            },
            "required": ["summary"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let given = input.get("text").and_then(Value::as_str).map(str::trim_end);
        let (draft, stands) = match (given.filter(|text| !text.is_empty()), self.handle.held()) {
            (Some(text), _) => (text.to_owned(), false),
            (None, Some(held)) => match held.submitted() {
                Some(text) => (text, false),
                None => (held.text, true),
            },
            (None, None) => return error_output("No draft is held: pass the answer as `text`."),
        };
        let Some(refusal) = self.handle.judge(&draft) else {
            if stands {
                self.handle.stand();
                let mut output =
                    text_output("The reply above stands as the answer; the turn ends.");
                output.result.terminate = Some(true);
                return output;
            }
            self.handle.accept(draft);
            return text_output("Answer delivered; the turn ends.");
        };
        let held = self.hold_file(&draft, context);
        let view = held.as_ref().map_or("", |(_, view)| view.as_str());
        let shown = format!("{refusal}\n{HELD}\n{view}");
        self.handle.hold(draft, held.map(|(file, _)| file));
        error_output(shown.trim_end().to_owned())
    }
}
