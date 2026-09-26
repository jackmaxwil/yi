# Yi prompts, skills, and workflow: ideation from the leak corpus and one bad session

Date: 2026-09-06. Tree at 0.163.0. Corpus: `ref/prompts/system_prompts_leaks` (fresh shallow clone, gitignored under ref/).
Supporting reports in the same directory: `session-autopsy.md`, `yi-inventory.md`, `anthropic.md`, `openai.md`, `grok-pi-others.md`. Every claim below has a file:line pointer in one of them.

> Superseded 2026-09-06 by docs/plans/2026-09-06-prompt-surface.md v2. Items A1 (a keyword-routed task class) and A4 (two prompt registers by model tier) were rejected by the user the same day: intent is the model's to read from a written method, and Yi has one prompt path. The rest was folded into the plan as exhaustive doctrine text.

Nothing here is implemented. This is the brainstorm the user asked for. Where an idea collides with a settled decision, the D-row is named so the collision is visible before code.

---

## 0. The session in one paragraph

Prompt: "analyze the yi repo comprehensively. rate it out of 10". Model: glm-5.3-flash. Twelve tool calls, 6m27s, 209K prompt tokens for a 20K context, $0.012. Five orientation calls took 16 s. Seven clippy/nextest calls took 236 s (93.5 % of tool time), four of them wasted on flags nextest does not have. Zero source files were read. The tests failed because the agent runs inside its own Seatbelt sandbox (`bind: PermissionDenied`), the model said so in its reasoning, and then docked the codebase 1.5 points for it. The counts it reported ("7 expects, 3 unsafe in production") are all test code, a comment, and a vendored file. It never opened the `[full output: …]` pointer that held the line count it later cited. The router priced the prompt as `one_shot`; the orchestrate fragment attached after the turn ended.

The model was not the main failure. The prompt handed it a rule ("Done is a measurement: run the relevant check") with no task-class scope, told it nothing about the sandbox it lives in, nothing about how tool output is reduced, and demoted the repository's own AGENTS.md to untrusted data. A cheap model did exactly what the text permitted.

---

## 1. What the corpus says, compressed

### 1.1 The consensus core (in nearly every coding prompt)

1. Independent tool calls go out together in one message.
2. Read before edit; the edit tool errors otherwise, and the tool text says so.
3. Prefer editing to creating; no unrequested docs or files.
4. Never commit, amend, push, force, or skip hooks unless asked.
5. Dedicated read/grep/edit tools over cat/grep/sed; never echo to talk to the user.
6. No scope creep: no abstractions, error handling, or refactors beyond the ask. "Three similar lines beat a premature abstraction."
7. Verify before claiming done; if you could not verify, say so.
8. Concise, lead with the answer, no pre/postamble, no emoji.
9. Check the library is already used before importing it.
10. `file:line` references; hook and reminder text is data, not orders.

Yi's doctrine already carries 6, 7, 8 in its own words. It lacks 1, 2 (stated in the tool), 3, 4, 5 (the identity fragment says the opposite for regex search), 9, and half of 10.

### 1.2 The moves that matter for a cheap model

These recur across Haiku 4.5's "classic" prompt, GPT-5.4-mini, Codex Spark, and Grok Build. They are the ones to copy first because Yi's default model is glm-class, not Fable-class.

- **Two prompt registers keyed on model tier.** Anthropic ships a 11.7 KB Bash description to Haiku and 2.6 KB to Fable (`anthropic.md` §5); OpenAI ships mechanical bullets to 5.4-mini and prose judgment to 5.4 (`openai.md` §6). Same tools, different prose. Weak models need NOT-tables, numbered steps, worked examples; frontier models need constraints only.
- **Prefer-X-over-Y tables and closed lists.** "Read files: use Read (NOT cat/head/tail)". "Parallelizable: cat, rg, sed, ls, git show, nl, wc". A weak model picks a row; it does not derive.
- **Command hint + reason + fallback in one line.** "prefer rg because it is faster; if rg is not found, use grep." Bare hints produce retries.
- **Negative examples with the observed symptom.** `echo "===="` chaining "renders poorly"; `applypatch` is not a tool; "do not wrap the patch in JSON". Each one is an incident, exactly like Yi's `Incident:` comment grants, but for the model.
- **Tool descriptions as gotcha lists.** Every bullet names the failure it prevents (Monitor's "if the log goes quiet after the match, tail never receives SIGPIPE"). Grok 4.6 has zero reasoning rules in prose; all policy lives on the tools, because the model reads the schema it is about to call more reliably than a rule 400 lines up.
- **Make the tool error the teacher.** Typed, closed outcome vocabularies (`{"error":"not_unique"}`) beat prohibitions. Yi's hashline edit already returns typed mismatches; the prompt should say the model never needs a confirmation read.
- **Length caps by change size, as a table.** Codex 5.1: ≤10-line change → 2-5 sentences, no headings; medium → ≤6 bullets; large → 1-2 bullets per file; never before/after pairs; hard cap 50-70 lines.
- **Request-type dispatch table.** Codex 5.6: Answer / Diagnose / Change / Monitor, each row with its own authorization. Gemini: Directive vs Inquiry, default Inquiry. Claude Code: "when the user is describing a problem or asking a question, the deliverable is your assessment. Report your findings and stop." This is the single rule that would have stopped the session above.
- **Honesty contract in position one.** Claude Code opens with "Report what actually happened, not what you intended. If you did not check, say you did not check." Weak models follow position-1 rules best.
- **Turn-cost model and early-stop criteria.** Gemini: "unnecessary turns are more expensive than wasted context". Amp: act once you can name exact files and symbols; do not Read the same file twice; prefer 200+ line reads over nibbles. Codex Spark is the extreme: read each file once, one patch phase, "prefer mistakes over over-exploration".
- **Compaction contract stated to the model.** "Time never runs out; do not restart from scratch; the newest message is steering, not a replacement objective."
- **Sentinel lines for machine-read state.** `result:` / `needs input:` / `failed:`; `::git-commit{}` only after success. Yi's ACP and headless paths can parse these instead of classifying prose.

### 1.3 Ideas unique to one vendor, worth stealing

- Pi generates the tool list and guidelines from the tool registry; the prompt cannot describe a tool that is absent. Yi's identity.md currently describes grep wrong for exactly this reason.
- Grok Build: "Minimum complexity means no gold-plating, not skipping the finish line." "Critical path is max(build, QA), not the sum." Shared contract before any parallel writes.
- Amp: verification scaled to blast radius ("a typo fix needs none"); AGENTS.md delivered after the first file operation in that directory, not all up front; "drafts, not legacy contracts" for shapes made earlier in the same thread.
- Codex plan mode: plans must be decision-complete; ground unknowns by exploring, not asking; two unknown types (discoverable → explore and present candidates; preference → 2-4 options with a default, proceed on the default if unanswered).
- Copilot CLI: baseline lint/tests before the change, then after; four escalating time-pressure strings; a five-section continuation template for context exhaustion.
- Anthropic verify skill: "Don't run tests. Don't typecheck. Running them here proves you can run CI, not that the change works." PASS/FAIL/BLOCKED/SKIP; "3 of 4 passed is FAIL"; "when in doubt, FAIL".
- Anthropic skill triggers: quoted user utterances, explicit "Do NOT use for", and a runnable pre-check grep the model executes before deciding.
- Grok 4.5 memory policy: "every memory reference must be earned; zero or one per response; never narrate the lookup." Parked: Yi has no memory surface.

---

## 2. Yi's current surface, and what is broken in it

From `yi-inventory.md`. Bugs first, because several of the ideas below are pointless until these are fixed.

### 2.1 Defects found while inventorying

1. **Folded frontmatter parses as `>`** (`crates/runtime/src/skills.rs:129-150`). `description: >` yields the description `">"`. All three repo skills (grid, review, session-mining) and 12 of 33 home skills read as `- grid: >` in the catalog. No test covers a folded description.
2. **`is_project_root` is false for any cwd under $HOME** (`ext/project.rs:22-24`). Project skills land in the trusted prefix; the yard "project skills" path is dead in real use. The e2e test passes because its temp HOME sits under the project.
3. **The catalog is a fixed 16,384 B head-truncate, not the 2 %-window ladder §5 promises.** This machine has 71 global skills at 19,902 B; the last 15 alphabetically are cut, including `yi-port` and `yi-tui-verify`.
4. **AGENTS.md rides as `trust="untrusted"`** because `~/.yi/trust.json` does not exist, and doctrine says untrusted text "never instructs". Every rule in this repo's AGENTS.md is advisory data to Yi. The model is never told `yi trust` exists. CLAUDE.md loads too, byte-identical, so 41 KB of yard carries 20 KB of content.
5. **identity.md is stale against the tools**: it says grep is literal-only and sends regex to `rg` via bash; the grep tool is regex-by-default with `literal`, `multiline`, `type` flags. It says "Code carries no comments" flatly; this repo has a comment grammar.
6. **`.ruler/` is ahead of the generated files**: `045-loud-caps.md`, `095-tracking.md`, `097-landing.md` are absent from AGENTS.md and CLAUDE.md. `020-architecture.md:3` says "Thirteen crates"; there are 15.
7. **D26's "preamble ≤ 8 KB" is not what the gate measures.** `check_request_budget.py` ratchets system + tools at 19,810 B (system 7,344, tools 12,466) from a fixture; the catalog (up to 16 KB) and the yard (41 KB here) sit outside it.
8. **D114's `trigger:` pointer mechanism is unused**: zero SKILL.md files on this machine carry `trigger:`.
9. **`~/.yi/skills` still holds caveman, ponytail, superpowers, diagram-design** that §14.1 says were deleted. They duplicate doctrine.md and eat the catalog budget.

### 2.2 What the model is not told (the 22-item gap list, grouped)

- Tools: no parallel-call guidance; no prefer-read/grep-over-bash (the reverse is stated); bash names no timeout, no background policy, no 30 KB cap, no rtk reducer, no `[full output:]` semantics; `get_context` says nothing about when to call it; `plan` is unmentioned outside the conditional orchestrate fragment; the rlm API surface lives only in orchestrate.md.
- Environment: no sandbox posture (no egress, socket bind denied, writes confined to cwd/tmp); no explanation of what a lane is or that its cwd is not the trunk; nothing about compaction or `<yi_compact_view>`.
- Workflow: no git rules; no ask-the-user channel (`ask_user` exists only for auto-review denials); no task-class dispatch; no length caps; no "do not re-read after edit"; no count hygiene.
- Repo law: the whole done-bar, testing doctrine, guardrails, and never-list live only in AGENTS.md, which is untrusted.

---

## 3. Ideation, track A: the system prompt

Ordered by expected effect on a glm-class model per byte added. D26 (8 KB preamble) is the constraint every item must respect; the plan is to *shrink* the prefix while adding these, by moving prose onto tools and cutting the yard.

### A1. Task-class dispatch, as data the router already computes

Yi has a deterministic prompt router (`ext_record route: one_shot | orchestrate`, score-based). Extend the closed vocabulary with an `inquiry` class keyed on verbs and shapes (analyze, rate, review, explain, why, should, how does, what is; a question mark with no imperative). The class attaches a 300-byte fragment:

> This turn is an assessment. Read, do not change. Evidence is what you read: code, docs, history, the repository's own gate records. Do not run the test suite or a lint gate to learn what CI already recorded; quote the record. A measurement that fails inside your sandbox (PermissionDenied, no network) is a fact about the sandbox, not the code. A grep count is not a finding until you have read the matches.

The four-row table (Answer / Diagnose / Change / Monitor from Codex 5.6, plus Assess) is state-space-as-data: the runtime reads it, the prompt renders the active row. This is a D-row: the router's vocabulary changes and the permission layer may deny mutation on inquiry turns by default. Reversible by deleting the row and the fragment.

Why this first: it is the one rule that converts the session above from 12 calls and 6 minutes into ~10 reads and one minute, and it costs under 400 bytes.

### A2. Honesty contract at position one, with the sandbox clause

Move a five-line "Reporting outcomes" block above identity's capability list. Content: report what happened, not what you intended; if you did not check, say so; a red gate is quoted verbatim; a check that fails on the sandbox's own denial is not evidence; never make a failure look resolved. The last clause did not exist anywhere in the corpus and is Yi-specific: Yi is the only agent in the set that sandboxes its own test suite.

### A3. Scope "Done is a measurement" to changes

One word fix in doctrine.md: "Run the relevant check *for a change you made* before claiming finished." Add the inverse: "For a question or assessment, the deliverable is your reading; report it and stop." (Claude Code F51:99.)

### A4. Two prompt registers keyed on model tier

`yi-ai`'s model catalog gains a `tier: classic | lean` column. `identity` and `doctrine` get a classic variant with NOT-tables, numbered steps, and two worked examples (one good/bad plan pair, one good/bad final answer pair). The lean variant is the current text, tightened. Budget: classic ≤ 12 KB, lean ≤ 6 KB, both under a new guardrail that measures the assembled prefix per tier (closes D26's missing gate). Design collision: D26 said one preamble; this is two. New D-row.

### A5. Generate the tool list from the registry (Pi's move)

Each `Tool` contributes `prompt_snippet` (one line) and `prompt_guidelines` (zero to three bullets). identity.md's hand-written tool paragraph is deleted; the assembled text lists only registered tools. The stale grep sentence becomes impossible. The `ToolAdapter` seam already exists; this is additive on it.

### A6. Tool descriptions become gotcha lists

bash (today 92 chars) gains: cwd persists, shell state does not; `&&` chains stop at the first nonzero segment and a `| head` closes the pipe with 141, so later segments silently never run; output over 30 KB is cut and output over 2 KB is reduced with `[N lines omitted]` and a `[full output: path]` you can `read`; `max_output_lines` raises the reducer budget; `wait` is clamped 5-300 s and a longer command becomes a job you check by calling bash with no command; inside auto mode an unparsable command runs contained (no network, no socket bind, writes to cwd and tmp only) and a PermissionDenied there says nothing about the code. read gains "do not re-read a file you just edited; the edit result already carries the new anchors." get_context gains "call once, first, on a repository you have not read this session." Each bullet is an incident from the session log, phrased as the failure it prevents.

### A7. Reduce contract: say what was cut

The reducer emits `[reduced: lines 100-140 omitted (wc, ls output)]` instead of `[N lines omitted]`, and the pointer line says `read this path for the rest`. A compound command whose later segments produced no bytes gets a host note: `segment 3 of 5 produced no output (pipeline status 141)`. This is code, not prompt, and it is the cheapest fix for "cited a number it never saw".

### A8. Environment block: sandbox and lane lines

Two lines, refreshed per turn like the rest: `sandbox: contained (no egress; unix bind denied; writes: cwd, tmp)` and `lane: worktree slot 1 of the trunk at <sha>; branch yi/<id>; the user lands it with /land`. The model then knows its cwd is not the trunk and why a socket test fails.

### A9. Repo instructions: trust, dedupe, and the rule text

Options, in ascending order of policy change:
- Tell the model `yi trust` exists, in the mode fragment, so an untrusted AGENTS.md is a known state rather than silence.
- Dedupe AGENTS.md and CLAUDE.md by content hash before fencing (saves 20 KB of yard today).
- Grant trust by default to instruction files at the git root of a repository the user owns (author email matches git config), which is the case for every repo this user works in. This is a D-row on the trust model.
- Amp's alternative: deliver the directory's AGENTS.md after the first file operation there, not up front. Cuts the yard to zero on turns that never touch the tree.

### A10. Voice: length caps and the assessment shape

The voice section is already the strongest fragment Yi has. Add the Codex 5.1 length table (three rows) and one row for assessments: "two paragraphs, then a table only if the dimensions are the user's, evidence column cites what was read". Ban the "(verified)" tic when the verification was a gate the repo already ran.

### A11. Parallel calls, git, ask channel, compaction

- One sentence: "Independent calls go out together; Yi runs them in order, so a call that depends on another's result waits."
- Git block in the mode fragment (not doctrine, so it rides the trusted block): never commit or push unasked; never `git add -A` in a shared tree; commit messages with backticks go through `git commit -F -`; the lane is the branch.
- Ask channel: promote `ask_user` from auto-review-only to always-registered, with Codex's shape (header, question, 2-4 options, a recommended default) and the rule "never write a multiple-choice question as prose". Pair with the Codex 6 timing rule: proceed on the default after the user is silent, and say so.
- Compaction paragraph in the context-management fragment, stated once: "compaction happens; the kernel survives; the newest message is steering, not a replacement objective; do not redo finished work."

### A12. What to cut to pay for the above

- The edit manual (5.2 KB) stays but moves to the classic tier only; lean gets a 1 KB version plus "the error text teaches the rest".
- The home copies of caveman/ponytail/superpowers/diagram-design (§14.1 already decided this).
- CLAUDE.md duplicate load.
- The catalog's 16 KB cap becomes the 2 % ladder with a shrink order (drop descriptions first, then names past the budget with a count line), never a silent alphabetical cut.

---

## 4. Ideation, track B: skills

Constraint from D42 and the memory file on deterministic loops: skills are human-authored, triggers are literal, no LLM-judged firing. D50: project-specific checks belong in a skill, not a deterministic advisor. That fits: every item below is a SKILL.md with a `trigger:` (D114's unused mechanism) plus a body shaped like Anthropic's (formula header, Phase 0 with exact commands, fixed output contract, degradation clause).

### B1. Mechanics first

- Fix the folded-description parser and add the test.
- Fix `is_project_root`.
- Put `trigger:` on every Yi skill so the pointer mechanism finally fires.
- Implement `$name` explicit invocation (§5 promise).
- Catalog ladder per A12.

### B2. Skills to write, in priority order

1. **`assess`** (codebase analysis, rating, review of a whole repo). Trigger: text `analyze|rate|assess|evaluate this (repo|codebase)`. Body: read ARCHITECTURE header and decision-log tail, YI_DESIGN §1, `git log --oneline -30`, CHANGELOG tail, the gate recipe, three source files chosen from the orientation packet (one boundary crate, one hot module, one test); quote CI state from the forge or the last merge; never run the suite; count hygiene rule; output contract: two paragraphs, dimensions table with an evidence column that cites what was read. This is the skill the session needed.
2. **`gate`** (this repository's test and check recipe). Trigger: tool:bash `cargo nextest|cargo test|just check|cargo clippy`. Body: `just check` is the gate and its exit code is the verdict; nextest filter syntax with the three forms that matter (`-p crate`, `-E 'binary(name)'`, `-E 'test(name)'`); `--no-fail-fast`; the list of tests that cannot pass inside the sandbox and why; "do not add `--quiet`". Fifty lines that would have saved four calls and three minutes.
3. **`verify`** (port of Anthropic's, adapted). Runtime observation is the evidence; running tests proves you can run CI; PASS/FAIL/BLOCKED/SKIP; "3 of 4 passed is FAIL"; a handle-discovery ladder that ends in `yi ask --model faux/faux-1` for offline and `yi tui --headless` for TUI changes.
4. **`debug`**. Doctrine already says reproduce first and one hypothesis at a time; the skill carries the commands: how to get a failing test, `RUST_BACKTRACE=1`, `--nocapture`, the faux cassette replay, the frame dump for TUI.
5. **`land`** (the forge flow, replacing the deleted `/land` per D122). Commit rules, `git commit -F -`, no assistant trailer, `just land` by parts, PR body template, `pulls/N/update` when behind base. The memory files on this flow already hold the incident list; the skill is where they belong.
6. **`plan`** methodology (Codex plan mode + Yi's plan tool). Decision-complete plans; ground unknowns by exploring; discoverable vs preference unknowns; the plan tool's forbidden transitions stated so the model does not fight the refusal; good/bad plan pair.
7. **`simplify`** (ponytail as a review pass over a diff). Formula: diff → four angles (reuse, dead code, altitude, dependency) → apply. Degradation: without `rlm.run`, do the angles sequentially.
8. **`port`** and **`tui-verify`** already exist; they just need to survive the catalog.
9. **`session-mining`** gains the new fingerprints from track C.
10. **`sandbox`** is not a skill; it is the environment line (A8).

### B3. Trigger craft, from the corpus

Quoted utterances, an explicit "Do NOT use for", and a runnable pre-check the model executes before deciding (the claude-api skill's grep). For weak models the pre-check is the most reliable trigger, and it is deterministic, which is what the rules engine wants.

---

## 5. Ideation, track C: pressure-testing the workflow

The session above was found by looking. The point of this track is that the next one is found by a script.

### C1. Fingerprints for the extractor

`skills/yi/session-mining/extract.py` gets deterministic fingerprints, each one an incident from this session:

- `gate_in_inquiry_turn`: a bash call matching the gate vocabulary in a turn whose route was `one_shot` and whose user text carried an inquiry verb.
- `flag_error`: a tool result with exit 2 and `unexpected argument`.
- `empty_filter`: nextest/pytest output containing `0 tests run`.
- `pointer_never_read`: a `[full output: path]` emitted and no later `read` of that path.
- `chain_stop`: a compound `&&` command whose later segments produced no bytes.
- `self_capped`: the model set `max_output_lines` and the result was under the cap.
- `count_claim`: a final message containing a number that appears in no tool result (heuristic, flagged not scored).
- `sandbox_denial_as_finding`: a result carrying `PermissionDenied` followed by a final message citing it as a property of the code.
- `cache_miss_streak`: consecutive calls with `cache_read: 0` on a stable prefix (this session: 8 of 13).

### C2. A prompt A/B harness on real cheap models

The faux provider cannot judge behavior; it replays. Pressure testing needs real runs. Proposal: `evals/journeys/` gets a prompt set (ten prompts: two assessments, two diagnoses, four changes, one monitor, one ambiguous) run against two or three models (glm-5.3-flash, one mid, one frontier) under prompt variant A and B. Scoring is deterministic from the session JSONL through the extractor: tool calls, wasted calls (the fingerprints above), gate runs in inquiry turns, reads before the first claim, wall time, cost, cache ratio, final-message length against the cap table. A human reads the final texts. The report is a table per prompt per model per variant. This is user-run, never scheduled, the same rule session-mining already carries.

### C3. Ratchets and baselines

- Assembled prefix bytes per tier, shrink-only.
- Behavior cassettes for the router: each class has a fixture prompt and an expected route; a change to the vocabulary that reroutes a fixture is red.
- The gate list that cannot pass inside the sandbox is a committed fixture the `gate` skill and the environment line both read, so they cannot drift apart.

### C4. Dogfood loop

Every Yi session on the yi repo is mined weekly by the user with `--mark`. A fingerprint that appears in two sessions becomes a proposal: a tool-description bullet, a skill line, or a router row. The proposal names the fingerprint; the fix names the session. That is the same incident-grant discipline the code comments already follow, applied to the prompt.

---

## 6. Screenshot notes (TUI, not prompt)

Minor, listed so they are not lost.

- Reasoning renders in full italics; a 1,700-character thinking block takes a screen. Collapse past three lines with a toggle.
- The tool card shows the command truncated at width with `…`, which hides the tail where the interesting part usually is (`| tail -6`, `wait: 300`). Wrap or show the tail.
- The result card shows one line (`Cargo.lock`). The user cannot tell that the model saw 57 lines and that the chain stopped. A `57 lines · reduced 1.3 KB · exit 0` summary row would.
- `⏎ 0` exit marker and per-call durations are good; the `12 tools · 6m 27s · 199K in / 2K out · 32% cached · $0.012` line is the best single UI element in the screenshots and is what made the autopsy easy.
- `PARTIAL - 4 of 6 layers` in the orientation packet is honest; the model did not act on it. The classic-tier prompt should say what to do when a packet is partial.

---

## 7. Suggested order

1. Fix the four defects (folded frontmatter, `is_project_root`, dedupe, ruler apply + "fifteen crates"). No design change, all measurable.
2. A1 + A3 + A2: the inquiry class, the scope word, the honesty block. One D-row. Re-run the session's prompt on glm-5.3-flash and compare with the autopsy numbers.
3. B2.1 and B2.2 (`assess`, `gate`) with `trigger:`. Re-run again.
4. A6 + A7 (tool gotchas, reduce manifest). Re-run.
5. C1 fingerprints, then C2 harness, so steps 6+ are measured rather than argued.
6. A4 tiers, A5 registry-generated tool text, A9 trust, the rest of B.

Each step ends with the same prompt on the same model and the extractor's table. The autopsy numbers are the baseline: 12 calls, 252 s of tool time, 0 source reads, 4 wasted calls, 1 sandbox denial reported as a finding.
