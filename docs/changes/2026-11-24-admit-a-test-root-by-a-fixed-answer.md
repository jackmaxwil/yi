---
issue: Closes #2346
raise: comments +3, crate runtime +6, tests +2
cut: crates/runtime/src/subagent.rs | pub fn with_admission(mut self, admission: Arc<AdmissionFn>) -> Self { | `admit_with` replaces it: a builder cannot reach a host `attach_runtime` has already shared
---
A commit is no longer refused because the machine is busy. `just commit` runs a test that wires a root the way `yi` does and spawns a child per role, and the host admitted each child only after reading the machine, so a 1-minute load average above 4 per processor failed the hook with "the host has no room for another session" whatever the commit held: four in a row on 2026-11-24, on a 16-processor Mac at a load of 90 to 160. A host can now be given its gate after `attach_runtime` hands it back (`SubagentHost::admit_with`, in place of `with_admission`, which only worked before the host was shared), and the shared test root gives it a fixed answer. Production still admits by the machine. Test files that build a host or a wiring by hand still read the machine, so the whole suite can still fail under load. #2346
