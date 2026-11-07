use std::path::Path;

use serde_json::{Map, Value, json};

use super::input::Patch;
use super::patcher::{PatchSectionResult, Patcher, SectionOp};
use super::tool::{HashlineState, SharedHashline, documents, lock_state};
use crate::diff::GitPatch;
use crate::ripwire::CheckLayer;
use crate::tool::{
    Tool, ToolContext, ToolKind, ToolOutput, error_output, require_str, resolve_path, text_output,
};

/// Incident: 182 byte-identical no-op edits on one path in 205 calls before an
/// abort; this many consecutive no-ops now escalate the soft hint to an error.
const NOOP_HARD_LIMIT: u32 = 3;

fn input_hash(input: &str) -> u64 {
    u64::from(xxhash_rust::xxh32::xxh32(input.as_bytes(), 0))
}

pub const EDIT_GUIDE: &str = include_str!("prompt.md");
const EDIT_DESCRIPTION: &str = "Edit existing files with a line-anchored patch (new files: write). A section opens `[PATH#TAG]`, TAG from the read/grep/write/edit output your line numbers came from. `PUT N.=M:` replaces original lines N-M with the `+TEXT` body rows under it; `PUT <N:` / `PUT >N:` insert before/after line N; `CUT N.=M` deletes. Every body row starts `+` and is final content: no `-` or context rows. Full guide (blocks, registers, moves, rules, examples): read(\"yi://tools/edit\") before your first edit.";
const GUIDE_POINTER: &str = "\nThe patch grammar: read(\"yi://tools/edit\").";

pub struct HashlineEditTool {
    pub state: SharedHashline,
    /// `edit.freeformGrammar`; the JSON argument is the default.
    pub freeform_grammar: bool,
}

impl Tool for HashlineEditTool {
    fn name(&self) -> &str {
        "edit"
    }

    fn description(&self) -> &str {
        EDIT_DESCRIPTION
    }

    fn schema(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {"type": "string", "description": "Hashline patch: [path#TAG] section headers followed by PUT/CUT/REM/MV ops and +TEXT body rows"}
            },
            "required": ["patch"]
        })
    }

    fn kind(&self) -> ToolKind {
        ToolKind::Write
    }

    /// The patch language as a Lark grammar; on providers with custom tools
    /// the body rows stop paying JSON escaping.
    fn freeform(&self) -> Option<yi_types::model::FreeformFormat> {
        self.freeform_grammar
            .then(|| yi_types::model::FreeformFormat {
                definition: include_str!("grammar.lark").to_owned(),
            })
    }

    /// [`Patcher::prepare`] validates and materializes the new text without touching disk.
    /// It runs on a clone of the store, so a refusal's reveal is recorded only by an execute.
    fn changes(&self, input: &Map<String, Value>, cwd: &Path) -> Vec<crate::tool::FileChange> {
        let Some(patch) = input.get("patch").and_then(Value::as_str) else {
            return Vec::new();
        };
        let Ok(patch) = Patch::parse(patch, Some(cwd)) else {
            return Vec::new();
        };
        let state = lock_state(&self.state);
        let mut snapshots = state.snapshots.clone();
        let mut scratch = super::clipboard::fork_clipboard(&state.clipboard);
        let mut patcher = Patcher::new(&mut snapshots, cwd.to_owned());
        let mut changes = Vec::new();
        for section in &patch.sections {
            let Ok(prepared) = patcher.prepare(section, &mut scratch) else {
                continue;
            };
            if prepared.is_noop() {
                continue;
            }
            let (before, after) = prepared.diff_inputs();
            changes.push(crate::tool::FileChange {
                path: yi_permission::resolve_target(prepared.path(), cwd),
                before: before.to_owned(),
                after: after.to_owned(),
            });
        }
        changes
    }

    fn preview(&self, input: &Map<String, Value>, cwd: &Path) -> Option<String> {
        let mut out = String::new();
        for change in self.changes(input, cwd) {
            out.push_str(crate::diff::patch(&change.before, &change.after, &change.path).as_str());
        }
        (!out.is_empty()).then_some(out)
    }

    fn execute(&self, input: Map<String, Value>, context: &ToolContext) -> ToolOutput {
        let patch_text = match require_str(&input, "patch") {
            Ok(patch) => patch.to_owned(),
            Err(message) => return error_output(message),
        };
        let parsing = yi_types::trace::span("edit.parse");
        let patch = match Patch::parse(&patch_text, Some(&context.cwd)) {
            Ok(patch) => patch,
            Err(message) => {
                return crate::tool::error_output_kind(
                    format!("{message}{GUIDE_POINTER}"),
                    yi_types::event::ToolErrorKind::InvalidArgs,
                );
            }
        };
        if patch.sections.is_empty() {
            return error_output(format!(
                "Patch input did not produce any sections.{GUIDE_POINTER}"
            ));
        }
        let home = documents(&self.state).map(|documents| documents.home);
        if let Some(refusal) = patch.sections.iter().find_map(|section| {
            crate::document::write_refusal(home.as_deref(), &resolve_path(context, &section.path))
        }) {
            return error_output(refusal);
        }
        let mut state = lock_state(&self.state);
        let HashlineState {
            snapshots,
            noop,
            clipboard,
            ..
        } = &mut *state;
        let mut host_clipboard = std::mem::take(clipboard);
        drop(parsing);
        let applying = yi_types::trace::span("edit.apply");
        let mut patcher = Patcher::for_call(snapshots, context);
        let results = patcher.apply(&patch, &mut host_clipboard);
        drop(applying);
        *clipboard = host_clipboard;
        let results = match results {
            Ok(results) => results,
            Err(message) => {
                let stale = message.starts_with(super::mismatch::EDIT_REJECTED_PREFIX);
                let grammar = message
                    .strip_prefix("line ")
                    .is_some_and(|rest| rest.starts_with(|c: char| c.is_ascii_digit()));
                let kind = if stale {
                    yi_types::event::ToolErrorKind::StaleTag
                } else {
                    yi_types::event::ToolErrorKind::ToolError
                };
                let pointer = if stale || grammar { GUIDE_POINTER } else { "" };
                return crate::tool::error_output_kind(format!("{message}{pointer}"), kind);
            }
        };
        let hash = input_hash(&patch_text);
        let mut rendered: Vec<String> = Vec::new();
        let mut diff = String::new();
        let mut checked: Vec<(usize, &str)> = Vec::new();
        for result in &results {
            if result.op == SectionOp::Noop {
                let entry = noop
                    .entry(result.canonical_path.clone())
                    .or_insert((hash, 0));
                if entry.0 == hash {
                    entry.1 += 1;
                } else {
                    *entry = (hash, 1);
                }
                if entry.1 >= NOOP_HARD_LIMIT {
                    return crate::tool::error_output_kind(
                        format!(
                            "Edit to {} was a byte-identical no-op {} times in a row. The file already contains this content — re-read it ({}) and issue a different edit, or stop editing.",
                            result.path, entry.1, result.header
                        ),
                        yi_types::event::ToolErrorKind::NoopLoop,
                    );
                }
                rendered.push(format!(
                    "{}\nno changes (the file already contains this content). Re-read the file before issuing another edit.",
                    result.header
                ));
                // An empty paste is a no-op whose warning is the only reason it changed nothing.
                rendered.extend(result.warnings.iter().map(|w| format!("warning: {w}")));
                continue;
            }
            noop.remove(&result.canonical_path);
            diff.push_str(
                result
                    .script
                    .patch(Path::new(&result.canonical_path))
                    .as_str(),
            );
            if result.op != SectionOp::Delete {
                checked.push((rendered.len(), result.canonical_path.as_str()));
            }
            rendered.push(super::messages::section_reply(result, snapshots));
        }
        // Invariant: the snapshot lock is released before the checker spawns, so a slow
        // checker does not serialize every other session's edit.
        drop(state);
        let (verdicts, layer) = post_edit_checks(&checked, &results, context);
        let mut syntax: Option<String> = None;
        for ((index, _), line) in checked.iter().zip(verdicts) {
            let Some(line) = line else { continue };
            if let Some(section) = rendered.get_mut(*index) {
                section.push('\n');
                section.push_str(&line);
            }
            syntax = Some(crate::syntax::worst(syntax, line));
        }
        let ripwire = layer.map_or("skipped", |layer| {
            rendered.push(layer.render());
            layer.name()
        });
        let mut output = text_output(rendered.join("\n\n"));
        output.value = context
            .program
            .then(|| super::values::edit_value(&results, &diff));
        if !diff.is_empty() {
            output.result.details = crate::diff::patch_details(&GitPatch::from_text(diff));
        }
        if let Value::Object(details) = &mut output.result.details {
            details.insert("ripwire".to_owned(), Value::String(ripwire.to_owned()));
            details.insert(
                "syntax".to_owned(),
                syntax.map_or(Value::Null, Value::String),
            );
            let count = |op: SectionOp| results.iter().filter(|result| result.op == op).count();
            details.insert(
                "ops".to_owned(),
                json!({
                    "updated": count(SectionOp::Update),
                    "deleted": count(SectionOp::Delete),
                    "noop": count(SectionOp::Noop),
                    "moved": results.iter().filter(|result| result.move_dest.is_some()).count(),
                }),
            );
        }
        output
    }
}

/// Both read the settled tree, so the ripwire check runs beside the syntax verdicts.
fn post_edit_checks(
    paths: &[(usize, &str)],
    results: &[PatchSectionResult],
    context: &ToolContext,
) -> (Vec<Option<String>>, Option<CheckLayer>) {
    let installed = crate::ripwire::installed();
    let top = {
        let root = crate::ripwire::root(&context.cwd);
        root.canonicalize().unwrap_or(root)
    };
    let updates: Vec<&PatchSectionResult> = (results.iter())
        .filter(|result| result.op == SectionOp::Update && installed)
        .collect();
    let regions: Vec<(String, u64)> = (updates.iter())
        .flat_map(|result| {
            crate::ripwire::regions(&top, &result.canonical_path, &result.script, &result.after)
        })
        .collect();
    let removed: Vec<(String, String)> = (updates.iter())
        .flat_map(|result| {
            let (path, before) = (&result.canonical_path, &result.before);
            crate::ripwire::removed(&top, path, before, &result.after)
        })
        .collect();
    std::thread::scope(|scope| {
        let ripwire = (!regions.is_empty() || !removed.is_empty()).then(|| {
            scope.spawn(|| {
                let _span = yi_types::trace::span("edit.ripwire");
                crate::ripwire::check(context, &top, regions, removed)
            })
        });
        let verdicts = paths
            .iter()
            .map(|(_, path)| crate::syntax::verdict(Path::new(path)))
            .collect();
        let layer = ripwire.map(|handle| {
            (handle.join()).unwrap_or_else(|_| CheckLayer::failed("the check's thread panicked"))
        });
        (verdicts, layer)
    })
}
