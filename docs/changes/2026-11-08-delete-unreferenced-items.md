---
issue: Closes #TBD
cut: crates/runtime/src/permission.rs | pub fn clear_holds(&self) { | no caller anywhere in src, tests, docs or scripts
cut: crates/runtime/src/plan/loop_coupling.rs | pub const LEDGER_TOOL: &str = "plan"; | no reference; LEDGER_CUSTOM_TYPE beside it is the one used
cut: crates/runtime/src/spec/gate.rs | pub const GREEN_STOP: &str = "spec_green_stop"; | no reference; the string appears nowhere else
cut: crates/runtime/src/todo/mod.rs | pub fn is_state_change(&self) -> bool { | no caller; only a frozen archive plan names it
cut: crates/mcp-cli/src/profiles.rs | pub fn value_url(value: &Value) -> Option<&str> { | no caller
cut: crates/tools/src/hashline/input.rs | pub fn with_path(&self, path: String) -> Self { | no caller
cut: crates/runtime/src/journal.rs | TailNotClean { path: PathBuf }, | never constructed or matched; no code checks the tail before an append, recovery sets a damaged tail aside first
cut: crates/runtime/src/plan/state.rs | UnknownKind { seq: Seq, op: String }, | never constructed; an unknown op kind already fails as ReduceError::Args when the op's arguments do not decode
cut: docs/prompts/2026-08-29-syntect-adoption-prompt.md | # Handoff prompt: plan syntect adoption (supersedes D63) | the adoption shipped (D74, D244, D262, D263); nothing refers to the prompt
---
Eight items with no reference anywhere in src, tests, docs or scripts are deleted, with the
syntect adoption handoff prompt: `PermissionBroker::clear_holds`, `LEDGER_TOOL`, `GREEN_STOP`,
`Op::is_state_change`, `value_url` (and its import), hashline `with_path`, `JournalError::TailNotClean`
and `ReduceError::UnknownKind`. The last was dead because an unknown op kind already fails as
`ReduceError::Args` when `op_of` cannot decode its arguments, not because op kinds are a closed
enum; `reduce` matches the kind as a string. Net -28 src lines; no behavior changes.
