---
name: never-relax-linters
description: "When a lint fails (clippy too_many_lines, -D warnings), fix the code, never allow/relax the lint or the gate"
metadata: 
  node_type: memory
  type: feedback
  originSessionId: 73f182c7-9bc2-4f1c-8ea6-d21f9ca364c5
  modified: 2026-09-02T04:12:36.994Z
---

When a gate fails on a lint, the user wants the code changed, not the lint. Said explicitly on 2026-09-02 about afterlife's `clippy::too_many_lines` under `-D warnings`: "Shorten functions. Never relax linter."

**Why:** the gates are the standard; loosening them to get green defeats the point of moving CI onto the forge.

**How to apply:** extract helpers, split functions, fix the finding. No `#[allow]`, no clippy.toml thresholds, no dropping `-D warnings`, no removing a lane. Offer options only when the fix would change behaviour. Related: [[deterministic-over-llm-loops]].
