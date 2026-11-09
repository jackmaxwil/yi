---
shares: #2124 | this branch adds ToolErrorKind::NotRun and moves tool-call execution out of run.rs; that PR changes how session events and stop reasons are kept, in other hunks of the same files
shares: #2123 | this branch moves tool-call execution into batch.rs and adds AgentTool::gates_message; that PR changes what is built per delta and request, in other hunks of the same files
shares: #2121 | both add a line to docs/YI_DESIGN.md in different sections; this one amends 1.2 and 4.2
shares: #2098 | this branch adds the /learn popup entry in input.rs and a docs/YI_DESIGN.md row; that PR sizes reasoning and tool-output windows, in other hunks
cut: .forgejo/workflows/tbv4-weekly.yml | name: | replaced by evals-weekly.yml, which judges main against last Monday's under hard caps instead of posting an unjudged row
cut: crates/loop/src/run.rs | struct ExtractedCall { | moved unchanged into crates/loop/src/batch.rs so the failed-write guard has one home
cut: docs/YI_DESIGN.md | automatic skill creation, | section 1.2 narrows its non-goal to unattended self-modification; refinement now lands only as owner-merged PRs
cut: evals/improve/round.py | #!/usr/bin/env python3 | the module moved to evals/refine/round.py, which drains every variant source into one gated round
cut: evals/levers.py | tokens = sum(value(row, k) for row in rows | the economy road now prices cost first in the unit the route bills, so the token-only sum is rewritten
cut: skills/yi/session-mining/extract.py | EXTRACTOR_VERSION = 6 | bumped to 7 with the harness census; the sessions-dir plumbing around it was rewritten, not removed
issue: Refs #2129
decision: Yi may change its own prompts, levers and code only through a refine variant that passes S0, the gate, and a better verdict at every stage, and lands as a pull request the owner merges; YI_DESIGN §1.2 now excludes unattended skill creation or self-modification instead of all refinement | the owner chose one gated loop over prompts and code with human-merged keeps, and the old exclusion forbade the loop the evals already ran by hand | restore the §1.2 wording and evals/improve from this change's parent
---
`just refine` replaces `just improve`, and `evals/improve/` moves to `evals/refine/`. `just refine propose <base> <binary>` still asks Yi, in a container that sees only development-task data, for up to three variants. `just refine round <base> <binary> --usd N` walks variants (proposed, given with `--variant`, or queued by `/learn` in `~/.yi/refine/queue.jsonl`) through a cascade: S0 refuses a path outside the surface, `crates/types/src` included, and a variant already rejected; a code variant builds in a worktree and must pass the gate before any paid measurement; then the economy suite screens it, the capability suite validates it on fresh seeds, and Terminal-Bench checks that it transfers. Every stage stores its verdict row, a stage the remaining budget cannot cover is not run, and only a variant better at every stage is kept, as a `refine/<hash>` branch for a pull request.

`skills/yi/refine/SKILL.md` teaches the same hill climb on any deliverable: a score that is a command, a frozen grader, one plan todo per variant, keep the best by the number, and stop at the budget.
