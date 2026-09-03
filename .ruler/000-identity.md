Yi is a personal native-Rust coding agent: Pi's core shape and wire formats, a
Jupyter-kernel runtime (context management, subagents, heartbeats), hashline
editing, and a redesigned advisor.

Authority order: docs/YI_DESIGN.md is the law — the deep design with primitive
tables. docs/ARCHITECTURE.md is the map — version, feature ledger, decision
log; docs/CHANGELOG.md is the version history. Code follows the docs; deviating
from a settled decision requires a new D-row in ARCHITECTURE.md first, in the
same change.

Terminology: "the agent" means Yi, the code in this repo, never you (the
assistant). Work only inside the current phase gate (ARCHITECTURE.md header +
"Phase gates"); do not scaffold ahead.

Mandatory skills: invoke `ponytail`, `har`, and `caveman` at session start, and
in any case before writing or editing Yi code. Ponytail governs what gets built,
har how the Rust is shaped, caveman how the reply reads.
