use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::{Map, Value, json};
use yi_types::image::image_defect;
use yi_types::kernel::{
    ExecuteResult, ExecuteStatus, NESTED_CALLS, NESTED_READS_DROPPED, NestedCall,
};
use yi_types::message::Content;

use crate::process::spill_path;
use crate::reduce::Involuntary::{ShellCell, Traceback};
use crate::reduce::view;
use crate::spill::{Spill, Stream};
use crate::tool::{
    CancelFlag, Tool, ToolContext, ToolKind, ToolOutput, detail_text, error_output, require_str,
    text_output,
};

pub struct KernelCellOutcome {
    pub result: ExecuteResult,
    pub kernel_restarted: bool,
    pub notes: Vec<String>,
}

/// yi-runtime implements this over yi-kernel; yi-tools never depends on it.
pub trait KernelBridge: Send + Sync {
    /// A stream cut for the model spills whole under `recovery_dir`, named in `notes`.
    fn execute_cell(
        &self,
        code: &str,
        call_id: &str,
        cancelled: &CancelFlag,
        recovery_dir: Option<&Path>,
    ) -> Result<KernelCellOutcome, String>;
}

type StreamSink = dyn FnMut(&str, &str) + Send;

/// A cell's stdout, stderr and displayed value as they arrived, and each one's characters (D316).
#[derive(Clone)]
pub struct CellSpill(Arc<Mutex<(Spill, [usize; 3])>>);

impl CellSpill {
    pub fn new(dir: Option<&Path>) -> Self {
        Self(Arc::new(Mutex::new((Spill::new(dir), [0; 3]))))
    }

    /// The kernel's uncapped `on_stream` hook.
    pub fn sink(&self) -> Box<StreamSink> {
        let spill = self.clone();
        Box::new(move |name, text| {
            let (slot, stream) = match name {
                "stdout" => (0, Stream::Out),
                "stderr" => (1, Stream::Err),
                "execute_result" => (2, Stream::Out),
                _ => return,
            };
            if let Ok(mut held) = spill.0.lock()
                && let Some(chars) = held.1.get_mut(slot)
            {
                *chars = chars.saturating_add(text.chars().count());
                held.0.write_to(stream, text.as_bytes());
            }
        })
    }

    /// The pointer when a stream or the displayed value passed `cap`.
    pub fn note(&self, cap: usize) -> Option<String> {
        let mut held = self.0.lock().ok()?;
        let cut = held.1.iter().any(|chars| *chars > cap);
        if cut { held.0.keep() } else { None }
    }
}

/// The calls a cell makes through the host, taken only while the cell is open.
pub trait CellCalls: Send + Sync {
    /// The bridge opens the cell once its execute `request` holds the kernel; only calls naming
    /// that request count.
    fn open(&self, request: &str);
    /// Closes the cell: its rows, and how many read rows it made past the kept count.
    fn close(&self) -> (Vec<NestedCall>, u64);
}

pub struct IpythonTool {
    pub bridge: Arc<dyn KernelBridge>,
    pub calls: Option<Arc<dyn CellCalls>>,
}

fn shell_cell(code: &str) -> Option<String> {
    let trimmed = code.trim_start();
    if let Some(body) = trimmed.strip_prefix("%%bash") {
        return Some(body.trim_start_matches(['\r', '\n']).to_owned());
    }
    let escaped: Vec<&str> = code
        .lines()
        .filter_map(|line| line.trim_start().strip_prefix('!'))
        .filter(|rest| !rest.starts_with('='))
        .collect();
    (!escaped.is_empty()).then(|| escaped.join("\n"))
}

impl Tool for IpythonTool {
    fn name(&self) -> &str {
        "ipython"
    }

    fn description(&self) -> &str {
        "Execute Python in this session's kernel: one process, yours alone, that boots on the first cell and keeps its variables across your calls and across compaction (a snapshot revives them; a restart after a hang says so and starts empty). Nothing here is shared with bash; the cwd is. `await` works at top level; `rlm` is preloaded (`help(rlm.run)`); `yi.tools.read(path=...)`, `yi.tools.edit(patch=...)` and the rest call your own tools from a cell, judged and recorded as your calls are; `%%bash` runs a shell in the kernel's env (on macOS loopback only, so `%pip install` fails); pandas (with openpyxl) reads spreadsheets, and `anydoc` and `pdf_inspector` (`extract_text`) read documents read cannot show. A cell's stdout, stderr or displayed value past 65,536 characters keeps its first and last halves, and the cut names [full output: path], a file with all of it (the first 256 MiB), which read opens; a cell is interrupted after 600 s, so split longer work across cells. Children run their own kernels: `rlm.status()` shows them, `rlm.put/get` and `kernel://<name>/<var>` move objects between kernels whole, and a child reads yours through `kernel://main/<var>`."
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": "Python scratchpad code or `%%bash` shell cells to execute in the agent kernel. The kernel is Yi's own venv over the machine's site-packages: what `python3` here imports, a cell imports; a project's own venv is reached through a `%%bash` cell that runs its interpreter or runner (`uv run`, `.venv/bin/python`, `cargo`, `npm`)."
                }
            },
            "required": ["code"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Exec
    }

    /// A shell cell reads through the same classifier as `bash`; Python is not
    /// statically readable, and the sandbox is what closes that residual.
    fn irreversible(&self, input: &Map<String, Value>) -> bool {
        let Some(code) = input.get("code").and_then(Value::as_str) else {
            return true;
        };
        match shell_cell(code) {
            Some(command) => yi_permission::verdict(&command) != yi_permission::Verdict::Allow,
            None => false,
        }
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let code = match require_str(&input, "code") {
            Ok(code) => code,
            Err(message) => return error_output(message),
        };
        let (cancelled, dir) = (&context.cancelled, context.recovery_dir.as_deref());
        let outcome = self
            .bridge
            .execute_cell(code, &context.call_id, cancelled, dir);
        let (rows, dropped) = self
            .calls
            .as_ref()
            .map(|calls| calls.close())
            .unwrap_or_default();
        let mut output = match outcome {
            Ok(outcome) => cell_output(code, outcome, dir),
            Err(message) => error_output(message),
        };
        if let Value::Object(details) = &mut output.result.details {
            if !rows.is_empty() {
                details.insert(NESTED_CALLS.to_owned(), json!(rows));
            }
            if dropped > 0 {
                details.insert(NESTED_READS_DROPPED.to_owned(), json!(dropped));
            }
        }
        output
    }
}

/// The distribution a `ModuleNotFoundError` names, top-level only: `a.b` installs as `a`.
fn missing_module(evalue: &str) -> Option<&str> {
    let rest = evalue.strip_prefix("No module named '")?;
    let name = rest.split('\'').next()?.split('.').next()?;
    (!name.is_empty()).then_some(name)
}

pub fn cell_output(code: &str, outcome: KernelCellOutcome, dir: Option<&Path>) -> ToolOutput {
    let (result, mut notes) = (outcome.result, outcome.notes);
    let mut sections = Vec::new();
    if !result.stdout.is_empty() {
        sections.push(result.stdout.clone());
    }
    if !result.stderr.is_empty() {
        sections.push(format!("stderr:\n{}", result.stderr));
    }
    if let Some(command) = shell_cell(code).filter(|_| !sections.is_empty()) {
        let kept = (notes.iter().find(|note| spill_path(note).is_some())).map(String::as_str);
        let exit = i32::from(result.error.is_some() || !code.trim_start().starts_with("%%bash"));
        let streams = sections.join("\n");
        let shell = view(ShellCell(&command), &streams, "", exit, kept, dir);
        notes.retain(|note| shell.recovery.as_ref() != Some(note));
        // Incident: with no file to name, the view's middle cut swallowed the kernel's cut row.
        let swallowed: Vec<String> = (streams.lines())
            .filter(|line| shell.recovery.is_none() && line.starts_with("[\u{2026} "))
            .filter(|line| !shell.text.contains(*line))
            .map(str::to_owned)
            .collect();
        sections = std::iter::once(shell.text).chain(swallowed).collect();
    }
    if let Some(value) = &result.result {
        sections.push(value.clone());
    }
    if let Some(error) = &result.error {
        sections.push(if error.traceback.is_empty() {
            format!("{}: {}", error.ename, error.evalue)
        } else {
            view(Traceback, &error.traceback.join("\n"), "", 1, None, dir).text
        });
        if error.ename == "ModuleNotFoundError"
            && let Some(name) = missing_module(&error.evalue)
        {
            sections.push(format!(
                "`{name}` is not installed in the kernel. On macOS the kernel has no network beyond loopback, so `%pip install {name}` fails there; ask the user to install it into the kernel's Python (`sys.executable`). `pip` in bash installs into a different Python."
            ));
        }
    }
    if result.status == ExecuteStatus::Aborted {
        sections.push("[cell aborted]".to_owned());
    }
    sections.extend(notes);
    let checked: Vec<_> = result
        .attachments
        .iter()
        .map(|attachment| {
            let defect = image_defect(&attachment.mime_type, &attachment.data);
            (attachment, defect)
        })
        .collect();
    let images = checked.iter().filter(|(attachment, defect)| {
        attachment.mime_type.starts_with("image/") && defect.is_none()
    });
    sections.extend(checked.iter().filter_map(|(attachment, defect)| {
        let defect = defect.filter(|_| attachment.mime_type.starts_with("image/"))?;
        Some(format!(
            "[{} attachment of {} base64 chars not sent to the model: the image {defect}. {}]",
            attachment.mime_type,
            attachment.data.len(),
            defect.remedy()
        ))
    }));
    let text = if sections.is_empty() {
        "(no output)".to_owned()
    } else {
        sections.join("\n")
    };
    let mut output = text_output(text);
    output
        .result
        .content
        .extend(images.map(|(attachment, _)| Content::Image {
            data: attachment.data.clone(),
            mime_type: attachment.mime_type.clone(),
        }));
    // The streams are carried apart from the joined text so a renderer can style stderr
    // and a traceback differently; the joined form stays the model's view.
    output.result.details = json!({
        "status": result.status,
        "durationMs": result.duration_ms,
        "attachments": result.attachments.len(),
        "attachmentMedia": checked.iter().map(|(attachment, defect)| if defect.is_none() {
            json!({ "mime_type": attachment.mime_type, "path": attachment.path })
        } else {
            json!(attachment)
        }).collect::<Vec<_>>(),
        "kernelRestarted": outcome.kernel_restarted,
        "code": detail_text(code),
        "stdout": detail_text(&result.stdout),
        "stderr": detail_text(&result.stderr),
        "result": detail_text(result.result.as_deref().unwrap_or_default()),
        "error": result.error,
    });
    output.is_error = result.status != ExecuteStatus::Ok;
    // A cell killed at the ceiling reads as any other error, so the graph could not carry a
    // next step for it and one F0e session wrote a second 600 s wait loop (#475).
    if result.status == ExecuteStatus::Aborted
        && let Value::Object(details) = &mut output.result.details
    {
        details.insert(
            "errorKind".to_owned(),
            Value::String(yi_types::event::ToolErrorKind::Aborted.as_str().to_owned()),
        );
    }
    output
}

#[cfg(test)]
mod tests {
    use super::missing_module;

    #[test]
    fn a_missing_module_names_its_distribution() {
        assert_eq!(missing_module("No module named 'xlrd'"), Some("xlrd"));
        assert_eq!(
            missing_module("No module named 'yaml.loader'"),
            Some("yaml")
        );
        assert_eq!(missing_module("name 'x' is not defined"), None);
    }
}
