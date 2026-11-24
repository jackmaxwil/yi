use std::sync::Arc;

use serde_json::{Map, Value, json};
use yi_tools::{Tool, ToolContext, ToolKind, ToolOutput, error_output, text_output};
use yi_types::acp::{AcpShowOutcome, AcpShowResult, AcpShowStrength};

/// Invariant: an address names a thing and is never the thing's text, so a longer one is a
/// mistake, refused with this cap named before it reaches the client.
const ADDRESS_MAX_BYTES: usize = 1024;

/// One thing the model points at for the attached client to show.
pub struct ShowAsk<'a> {
    /// True once the asking session is aborted: the shower gives up the wait.
    pub stop: Option<&'a yi_tools::CancelFlag>,
    pub address: &'a str,
    /// The session's working directory, which a relative path is under.
    pub cwd: &'a std::path::Path,
    pub strength: AcpShowStrength,
}

/// The attached client's display; `None` when none answered in time. A session has one only
/// when its client said it can show things, and only then is the tool offered.
pub type Shower = Arc<dyn Fn(&ShowAsk<'_>) -> Option<AcpShowResult> + Send + Sync>;

/// What the front end hands a session before its runtime is attached; none by default.
#[derive(Default)]
pub(crate) struct FrontEnd {
    pub(crate) rewriter: std::sync::Mutex<Option<Arc<crate::rewrite::Rewriter>>>,
    pub(crate) shower: std::sync::Mutex<Option<Shower>>,
}

struct ShowTool {
    shower: Shower,
}

/// The `show` tool over `shower`; the wiring pushes it for a session that has one.
pub fn tool(shower: Shower) -> Arc<dyn Tool> {
    Arc::new(ShowTool { shower })
}

fn strength(input: &Map<String, Value>) -> Result<AcpShowStrength, String> {
    match input.get("strength").map(Value::as_str) {
        None | Some(Some("tab")) => Ok(AcpShowStrength::Tab),
        Some(Some("peek")) => Ok(AcpShowStrength::Peek),
        Some(Some("front")) => Ok(AcpShowStrength::Front),
        Some(_) => Err("`strength` is one of `peek`, `tab`, `front`.".to_owned()),
    }
}

fn address(input: &Map<String, Value>) -> Result<&str, String> {
    let address = input.get("address").and_then(Value::as_str).map(str::trim);
    match address.filter(|address| !address.is_empty()) {
        None => Err("`address` is a path, `path:LINE` or `yi://file/PATH#LLINE`.".to_owned()),
        Some(address) if address.len() > ADDRESS_MAX_BYTES => Err(format!(
            "`address` is {} bytes; ADDRESS_MAX_BYTES is {ADDRESS_MAX_BYTES}. Pass the path, not its text.",
            address.len()
        )),
        Some(address) => Ok(address),
    }
}

/// What the model reads back: the outcome in a sentence, then the client's own note.
fn said(result: Option<AcpShowResult>) -> ToolOutput {
    let Some(AcpShowResult { outcome, note }) = result else {
        return text_output("No display answered; nothing was shown. Carry on without it.");
    };
    let note = note.map_or_else(String::new, |note| format!(" {note}"));
    match outcome {
        AcpShowOutcome::Front => text_output(format!("Shown in front.{note}")),
        AcpShowOutcome::Tab => text_output(format!(
            "Added as a tab; the user was not interrupted.{note}"
        )),
        AcpShowOutcome::Peek => text_output(format!("Shown as a peek.{note}")),
        AcpShowOutcome::Refused => error_output(format!("Not shown.{note}")),
        AcpShowOutcome::Other(other) => {
            text_output(format!("The display answered `{other}`.{note}"))
        }
    }
}

impl Tool for ShowTool {
    fn name(&self) -> &str {
        "show"
    }

    fn description(&self) -> &str {
        "Put a file in front of the user on their display, optionally at a line. `peek` and `tab` never interrupt them; `front` comes forward only if they are idle, else it waits as a tab. The result says which happened. Point at what you want read; do not show every file you touch."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "address": {"type": "string", "description": "A path, `path:LINE`, or `yi://file/PATH#LLINE`."},
                "strength": {"type": "string", "enum": ["peek", "tab", "front"], "description": "Default `tab`."}
            },
            "required": ["address"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Read
    }

    fn validate(&self, input: &Map<String, Value>) -> Result<(), String> {
        address(input).and(strength(input)).map(drop)
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let asked = address(&input).and_then(|address| Ok((address, strength(&input)?)));
        let (address, strength) = match asked {
            Ok(asked) => asked,
            Err(refusal) => return error_output(refusal),
        };
        let ask = ShowAsk {
            stop: Some(&context.cancelled),
            address,
            cwd: &context.cwd,
            strength,
        };
        said((self.shower)(&ask))
    }
}

impl crate::session::AgentSession {
    /// Set before the runtime is attached, for a session whose client can show things: the
    /// `show` tool joins the frozen tool set only then.
    pub fn set_shower(&self, shower: Option<Shower>) {
        if let Ok(mut slot) = self.front.shower.lock() {
            *slot = shower;
        }
    }

    pub(crate) fn shower(&self) -> Option<Shower> {
        self.front.shower.lock().ok().and_then(|slot| slot.clone())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use yi_types::message::join_text;

    use super::*;

    /// A display that answers `answer` and keeps what it was asked.
    fn display(answer: Option<AcpShowResult>) -> (Arc<dyn Tool>, Arc<Mutex<Vec<String>>>) {
        let asked = Arc::new(Mutex::new(Vec::new()));
        let seen = Arc::clone(&asked);
        let shower: Shower = Arc::new(move |ask: &ShowAsk<'_>| {
            let line = format!("{} {:?} {}", ask.address, ask.strength, ask.cwd.display());
            seen.lock().ok()?.push(line);
            answer.clone()
        });
        (tool(shower), asked)
    }

    fn call(tool: &Arc<dyn Tool>, input: &Value) -> (String, bool) {
        let input = input.as_object().cloned().unwrap_or_default();
        let context = ToolContext::new(std::path::PathBuf::from("/repo"));
        let output = tool.execute(input, &context);
        (join_text(&output.result.content, "\n"), output.is_error)
    }

    fn answer(outcome: AcpShowOutcome, note: Option<&str>) -> Option<AcpShowResult> {
        let note = note.map(str::to_owned);
        Some(AcpShowResult { outcome, note })
    }

    /// The display is asked for the address as written, under the session's folder, as a
    /// tab unless the model said otherwise; what it answers is what the model reads.
    #[test]
    fn the_display_is_asked_as_written_and_its_answer_is_read_back() -> Result<(), String> {
        let (tool, asked) = display(answer(AcpShowOutcome::Tab, Some("Busy there.")));
        let said = call(&tool, &json!({"address": " src/a.rs:7 "}));
        let read = "Added as a tab; the user was not interrupted. Busy there.";
        assert_eq!(said, (read.to_owned(), false));
        call(&tool, &json!({"address": "src/a.rs", "strength": "front"}));
        call(&tool, &json!({"address": "src/a.rs", "strength": "peek"}));
        let asked = asked.lock().map_err(|error| error.to_string())?;
        let want = [
            "src/a.rs:7 Tab /repo",
            "src/a.rs Front /repo",
            "src/a.rs Peek /repo",
        ];
        assert_eq!(*asked, want);
        Ok(())
    }

    /// Each outcome in its own words; a refusal is an error and no answer is not one, since
    /// the model should carry on either way but only one of them is a thing it got wrong.
    #[test]
    fn each_outcome_reads_in_its_own_words() {
        let read = |answer| call(&display(answer).0, &json!({"address": "a.rs"}));
        let front = read(answer(AcpShowOutcome::Front, None));
        assert_eq!(front, ("Shown in front.".to_owned(), false));
        let peek = read(answer(AcpShowOutcome::Peek, None));
        assert_eq!(peek, ("Shown as a peek.".to_owned(), false));
        let refused = read(answer(AcpShowOutcome::Refused, Some("No such file.")));
        assert_eq!(refused, ("Not shown. No such file.".to_owned(), true));
        let other = read(answer(AcpShowOutcome::Other("pinned".to_owned()), None));
        assert_eq!(other, ("The display answered `pinned`.".to_owned(), false));
        let (said, error) = read(None);
        assert!(said.starts_with("No display answered") && !error, "{said}");
    }

    /// A call the display should never see: no address, a strength that is none of the
    /// three, and an address one byte past the cap, which the refusal names.
    #[test]
    fn a_bad_call_is_refused_before_the_display_is_asked() -> Result<(), String> {
        let (tool, asked) = display(answer(AcpShowOutcome::Front, None));
        let at_cap = "a".repeat(ADDRESS_MAX_BYTES);
        assert!(
            !call(&tool, &json!({"address": at_cap})).1,
            "the cap itself passes"
        );
        let past = call(&tool, &json!({"address": format!("{at_cap}a")}));
        assert!(
            past.1 && past.0.contains("1025 bytes; ADDRESS_MAX_BYTES is 1024"),
            "{past:?}"
        );
        for bad in [json!({}), json!({"address": "  "}), json!({"address": 7})] {
            assert!(call(&tool, &bad).1, "{bad}");
        }
        let loud = call(&tool, &json!({"address": "a.rs", "strength": "loud"}));
        assert!(
            loud.1 && loud.0.contains("`peek`, `tab`, `front`"),
            "{loud:?}"
        );
        assert_eq!(asked.lock().map_err(|error| error.to_string())?.len(), 1);
        Ok(())
    }
}
