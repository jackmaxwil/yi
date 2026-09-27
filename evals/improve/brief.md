You are proposing one change to Yi, the coding agent whose source tree is the current directory (/work), so that it scores higher on Terminal-Bench tasks or spends less doing so.

What you can read:
- /corpus/rows.jsonl: one JSON row per recorded benchmark trial of the development tasks: `task`, `reward` (0 or 1), `partialScore` (the verifier's partial credit, 0 to 1), `turns`, token columns, `costUsd`, `censored` (stopped past $1 or 180 turns).
- /corpus/sessions/<trial>/*.jsonl: those trials' full session files, one JSON entry per line: every model turn, tool call and tool result.
- The source tree, to find where a behavior you saw comes from.

What you may change (anything else is refused before it runs):
- the prompt assets under crates/runtime/src/prompts/ (identity.md, doctrine.md, graph.json, and the others there);
- tool descriptions and refusal texts in crates/tools/src/hashline/prompt.md, crates/tools/src/builtins.rs, crates/tools/src/grep.rs, crates/tools/src/orient.rs, crates/runtime/src/todo/text.rs;
- integer levers listed as tunable in evals/levers/levers.json, by naming them in `levers` below instead of editing any file.

How to work:
1. Read rows.jsonl and find where trials lose partial credit, run out of turns, or spend the most. Open the sessions behind those rows and find one concrete, repeated failure: the same wasted step, refusal, spiral or misunderstanding in more than one trial.
2. Find the text or lever that causes or could prevent it.
3. Make exactly one small change that addresses it. Prefer deleting or tightening text over adding it: every byte of the prompt is paid on every turn.
4. Write /work/.candidate.json: {"target": "<the failure, in one sentence, with the trials you saw it in>", "rationale": "<why this change should fix it>", "levers": {<name>: <integer>, ...} or {}}.

Stop after writing .candidate.json. Do not run the benchmark, do not build the project, and do not describe the change anywhere but in .candidate.json.
