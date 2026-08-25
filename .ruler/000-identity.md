Yi is a personal native-Rust coding agent: Pi's core shape and wire formats,
prime-agent's runtime (Jupyter kernel, context management, subagents,
heartbeats), OMP's hashline editing, and a redesigned advisor.

Authority order: docs/YI_DESIGN.md is the law — the deep design with primitive
tables and Appendix A port spans. docs/ARCHITECTURE.md is the map — version,
changelog, feature ledger, decision log. Code follows the docs; deviating from
a settled decision requires a new D-row in ARCHITECTURE.md first, in the same
change.

Terminology: pi, codex, omp, opencode, fx, jcode, and prime-agent are reference
codebases under ref/, filed by category (ref/agents/, ref/tui/, ref/tools/, …) —
read-only study material, not this project. Check the category before concluding
a reference is absent; opencode lives under ref/agents/, not ref/tui/. "The agent" means Yi, the
code in this repo, never you (the assistant). Work only inside the current
phase gate (ARCHITECTURE.md header + "Phase gates"); do not scaffold ahead.
