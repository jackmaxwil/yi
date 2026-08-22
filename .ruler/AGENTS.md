Yi is a personal native-Rust coding agent: Pi's core shape and wire formats, prime-agent's
runtime (Jupyter kernel, context management, subagents, heartbeats), OMP's hashline editing, a
redesigned advisor, and an AA-benchmark-native harness.

Authority order: docs/YI_DESIGN.md is the law — the deep design with primitive tables and
Appendix A port spans. docs/ARCHITECTURE.md is the map — version, changelog, feature ledger,
decision log D1–D31, phase gates. Code follows the docs; deviating from a settled decision
requires a new D-row in ARCHITECTURE.md first, in the same change.

Work only inside the current phase gate (ARCHITECTURE.md header + "Phase gates"); do not
scaffold ahead of it.
