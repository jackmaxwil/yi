---
issue: Closes #1472, Refs #552, Refs #1442
---
The duplication gate now reads prompts and shipped skills as well as Rust. It scanned only `crates/*/src/**/*.rs`, although the guardrail rules said production duplication covers prompts and the skills rule said the gate refuses a skill that restates a tool description; it now also scans `crates/*/src/**/*.md`, `skills/**/SKILL.md` and `python/skills/**/SKILL.md` with a 6-line window, and skips the `yi:schema` blocks that are rendered from a tool's schema and so repeat by design. Today's tree has no prose copy that long. Its new `--selfcheck` plants a 6-line copy across two skills, a 5-line one and a repeated schema block, and fails if the file set, the schema skip or the window width is switched off. Units: B4-dup-gate-prose.
