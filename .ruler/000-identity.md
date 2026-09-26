Yi is a personal native-Rust coding agent: one `yi` binary, a persistent Jupyter-kernel
runtime (subagents, mailbox, plans, heartbeats), hashline editing, worktree lanes, and an
advisor. Its sessions are stored in Pi's v4 session JSONL format.

Authority order: docs/YI_DESIGN.md is the law — the deep design with primitive
tables. docs/ARCHITECTURE.md is the map — version, feature ledger, decision
log; docs/CHANGELOG.md is the version history. Code follows the docs; deviating
from a settled decision requires a new D-row in ARCHITECTURE.md first, in the
same change.

Terminology: "the agent" means Yi, the code in this repo, never you (the
assistant). Open work lives on the forge (095); do not scaffold ahead of an issue.

Mandatory skills: invoke `ponytail`, `har`, and `caveman` at session start, and
in any case before writing or editing Yi code. Ponytail governs what gets built,
har how the Rust is shaped, caveman how the reply reads.
