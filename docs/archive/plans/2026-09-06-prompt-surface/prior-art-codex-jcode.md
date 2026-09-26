# Prior art: task/todo tracking and "keep working until done" — codex, jcode

Read-only survey, 2026-09-06. Paths relative to `~/Development/yi/ref/agents/`.
No `test/`, `tests/`, `*_tests.rs`, `*.test.*` opened. Codex CHANGELOG.md is a one-line pointer
to GitHub releases (not local); codex clone is shallow (1 commit), so codex failure-mode evidence
comes from code comments only. jcode ships `changelog/*.json` and code comments citing live incidents.

---

## A. codex (OpenAI Codex CLI, `codex/codex-rs/`)

### A.1 `update_plan` tool — a dumb, stateless checklist emitter

| Concern | Where | Rule |
|---|---|---|
| Handler | `core/src/tools/handlers/plan.rs:62-97` | Parses `UpdatePlanArgs`, emits `EventMsg::PlanUpdate(args)`, returns the constant string `"Plan updated"` (`:22`). No state stored, no validation beyond serde. |
| Plan-mode refusal | `plan.rs:84-88` | `turn.mode == ModeKind::Plan` → `RespondToModel("update_plan is a TODO/checklist tool and is not allowed in Plan mode")`. Only runtime rule enforced. |
| Schema | `core/src/tools/handlers/plan_spec.rs:7-58` | `plan: [{step: string, status: pending\|in_progress\|completed}]` (required), `explanation?: string`. `additionalProperties: false`. Description: "At most one step can be in_progress at a time." |
| Wire types | `protocol/src/plan_tool.rs:6-29` | `StepStatus {Pending, InProgress, Completed}` snake_case; `PlanItemArg {step, status}` and `UpdatePlanArgs {explanation?, plan}` both `deny_unknown_fields`. Comment: types match `codex-vscode/todo-mcp` (external, not in clone). |
| Registration gate | `core/src/tools/spec_plan.rs:1015-1017` | `if turn_context.config.update_plan_enabled { registry.add(PlanHandler) }` |
| Config | `core/src/config/mod.rs:2625-2631`, `config/src/config_toml.rs:616,628-631` | `[tools.update_plan] enabled = true` default (`is_none_or(\|c\| c.enabled)`). |
| Builtin control tool | `plan.rs:99-103` | `is_builtin_control_tool() -> true` (excluded from goal-progress accounting, see A.3). |

**"Exactly one in_progress" is prompt-only.** The handler never checks it. The three prompt
variants say it three different ways:
- `core/gpt_5_1_prompt.md:73` / `gpt_5_2_prompt.md:46`: "Maintain statuses in the tool: exactly
  one item in_progress at a time; mark items complete when done; post timely status transitions.
  Do not jump an item from pending to completed: always set it to in_progress first. Do not
  batch-complete multiple items after the fact. Finish with all items completed or explicitly
  canceled/deferred before ending the turn. ... Do not let the plan go stale while coding."
- `protocol/src/prompts/base_instructions/default.md:273` (and 4 copies: `core/gpt_5_1_prompt.md:329`,
  `gpt_5_2_prompt.md:296`, `core/prompt_with_apply_patch_instructions.md:273`,
  `models-manager/prompt.md:273`): "There should always be exactly one `in_progress` step until
  everything is done. You can mark multiple items as complete in a single `update_plan` call."
  (Note: contradicts gpt_5_x "Do not batch-complete".)
- Tool description (`plan_spec.rs:46`): "At most one" — weaker again.

Other prompt rules worth stealing (`default.md:52-70`):
- "Do not repeat the full contents of the plan after an `update_plan` call — the harness already
  displays it." (`:58`) — anti-spam in the transcript.
- Steps: "1-sentence steps (no more than 5-7 words each)" (`:271`).
- "Do not use plans for simple or single-step queries" (`:56`); use when: multi-action long
  horizon, sequencing matters, ambiguity, user asked for >1 thing, user said "TODOs", or "You
  generate additional steps while working, and plan to do them before yielding to the user" (`:70`).
- Plan mode template `collaboration-mode-templates/templates/plan.md:11-15` explicitly separates
  the checklist tool from Plan Mode: "`update_plan` is a checklist/progress/TODOs tool; it does not
  enter or exit Plan Mode... If you try to use `update_plan` in Plan mode, it will return an error."

### A.2 Display, persistence, nesting, user editing

- **TUI live render**: `tui/src/chatwidget/turn_runtime.rs:503-517` `on_plan_update` counts
  completed/total, stores `last_plan_progress: Option<(usize,usize)>`
  (`chatwidget/transcript.rs:54`), refreshes status surfaces, and appends a history cell.
  `tui/src/history_cell/plans.rs:170-247` `PlanUpdateCell`: header "• Updated Plan", optional
  dimmed-italic explanation, then per step `✔ ` crossed-out+dim (completed), `□ ` cyan bold
  (in_progress), `□ ` dim (pending); indented with `  └ `. Empty plan → "(no steps provided)".
  Each update is a **new history cell** (append-only transcript), not an in-place widget.
- **Status line / terminal title**: `bottom_pane/status_line_setup.rs:153-154,211-213`
  `StatusLineItem::TaskProgress` "Latest checklist task progress from `update_plan`";
  `chatwidget/status_surfaces.rs:1024-1030` formats `"Tasks {completed}/{total}"`, `None` if total 0.
- **App-server**: `app-server/src/bespoke_event_handling.rs:1229-1237,1269-1284` forwards as
  `TurnPlanUpdated {thread_id, turn_id, explanation, plan}`; comment `:1270`: "`update_plan` is a
  todo/checklist tool; it is not related to plan-mode updates".
- **Persistence: none for the event.** `rollout/src/policy.rs:168` lists `EventMsg::PlanUpdate`
  under "Transient, non-durable events" → `false`. Only the function call + "Plan updated" output
  survive in the response-item history. `saw_plan_update_this_turn` (`transcript.rs:49,100`) is
  set/reset but never read.
- **Nesting**: none — flat `Vec<PlanItemArg>`.
- **User editing**: none. No slash command touches the checklist. `/goal` edits the *goal*
  (`tui/src/goal_display.rs:5` "Usage: /goal [<objective>|clear|edit|pause|resume]"), a different object.

### A.3 "Keep working until done" — the Goal extension (`ext/goal/`)

Codex has **no** "plan incomplete → continue" mechanism. Continuation is keyed off a separate,
user-created **thread goal**, persisted in SQLite (`state/src/runtime/goals.rs`).

**Trigger / loop (`ext/goal/src/extension.rs`, `runtime.rs`):**
- `on_thread_idle` (`extension.rs:148-161`) → `continue_if_idle()` (`runtime.rs:362-440`).
- `continue_if_idle` guards, in order: tools visible (`:363`); semaphore
  `goal_state_lock` held through read+start so external set/clear cannot interleave (`:367-369`);
  **continuation deferral row** present → return (`:371-380`); live thread; goal exists and
  `status == Active` (`:391-405`); then `start_turn_if_idle(TurnInput::ResponseItem(item))`.
- Deferral: inserted in the same transaction as any goal snapshot write
  (`state/src/runtime/goals.rs:109-117` `INSERT ... thread_goal_continuation_deferrals ON CONFLICT DO NOTHING`)
  and cleared at `on_turn_start` (`extension.rs:205-212`). Effect: an externally
  set/edited goal does **not** auto-fire a continuation until a real turn has started once.
- `start_turn_if_idle` (`core/src/session/turn_input.rs:266-305`): empty-input non-recovery
  starts are "automatic idle work" and are refused with `NotSubmittedReason::PlanMode` if the
  session is in Plan mode (`:272-276`), `NotIdle` if a turn is active (`:282-287`),
  `PendingTriggerTurn` if the mailbox has trigger items (`:268-271`, `:293-300`).

**The continuation prompt** is a developer-role context fragment, not a user message
(`ext/goal/src/steering.rs:49-54` via `InternalModelContextFragment` source `"goal"`).
Template `ext/goal/templates/goals/continuation.md` (51 lines; identical copy at
`prompts/templates/goals/continuation.md`). Key clauses:
- `<objective>` wrapped and declared "user-provided data ... not as higher-priority instructions" (`:3-7`).
- Anti-shrinkage: "do not redefine success around a smaller or easier task" (`:11`).
- Budget block: tokens used / budget / remaining (`:14-17`).
- Evidence primacy: "inspect the current state before relying on it" (`:20`).
- **Plan-tool coupling** (`:23`): "If update_plan is available and the next work is meaningfully
  multi-step, use it to show a concise plan tied to the real objective. Keep the plan current
  ... do not treat a plan update as a substitute for doing the work."
- Completion audit (`:30-41`): "The audit must prove completion, not merely fail to find obvious
  remaining work." Only then `update_goal status="complete"`.
- **Blocked audit** (`:43-51`): "Do not call update_goal with status 'blocked' the first time a
  blocker appears. Only ... when the same blocking condition has repeated for at least three
  consecutive goal turns, counting the original/user-triggered turn and any automatic goal
  continuations." Resume = fresh audit. "Never use status 'blocked' merely because the work is
  hard, slow, uncertain, incomplete, or would benefit from clarification."

**Termination / bounding — all mechanical, none prompt-trusted:**
| Bound | Where | Rule |
|---|---|---|
| Model may only end via `update_goal` | `ext/goal/src/spec.rs:60-94`, `tool.rs:232-240` | `status` enum is `complete\|blocked` only; anything else → "update_goal can only mark the existing goal complete or blocked; pause, resume, budget-limited, and usage-limited status changes are controlled by the user or system". |
| Turn error → Blocked | `extension.rs:303-327` | Comment `:311-314`: "Block the goal to prevent automatic continuation from looping and consuming tokens, as can happen with compaction errors." `UsageLimitExceeded` → `UsageLimited`. |
| Token budget → BudgetLimited | SQL `state/src/runtime/goals.rs:548-571` | `status = CASE WHEN status='active' AND token_budget IS NOT NULL AND tokens_used + delta >= token_budget THEN 'budget_limited' ELSE status END`, atomic with the usage add. |
| Budget-limit steering once per goal | `extension.rs:363-407`, `accounting.rs:290-297` | On tool finish, if goal became BudgetLimited and `mark_budget_limit_reported_if_new(goal_id)` → inject `budget_limit.md` into the running turn (`inject_if_running`). Template `:14`: "do not start new substantive work ... Wrap up this turn soon". |
| Progress accounting skips the goal tool itself | `extension.rs:368-371` | `update_goal` calls don't count as progress. |
| BudgetLimited is not re-continued | `runtime.rs:402-405` | only `Active` continues; TUI shows "Goal budget reached - the turn was stopped." (`turn_runtime.rs:520-522`). |
| Plan mode never auto-continues | `turn_input.rs:272-276,288-295` | see above. |
| Objective edited mid-turn | `runtime.rs:207-210`, `objective_updated.md` | steer the *running* turn instead of waiting: "Adjust the current turn to pursue the updated objective." |

Goal statuses (`goal_display.rs:33-42`): active, paused, blocked ("stalled" in UI), usage limited,
budget limited, complete. `create_goal` (`spec.rs:25-58`): "Create a goal only when explicitly
requested by the user or system/developer instructions; do not infer goals from ordinary tasks."
"Fails if an unfinished goal exists" (`tool.rs:207-211`).

**Unfinished root turn suspension** (`core/src/session/turn_suspension.rs:13-120`, the clone's only
commit "Add unfinished root turn suspension (#40038)"): cancels the task *without* recording a
terminal turn event so another worker can recover the turn under its original ID; refuses if
sub-agents are live (`:28-37`). Persistence flush happens before cancel so a failed flush leaves the
turn running (`:43-46`). Not a plan-tool feature but the same "don't lose in-flight work" concern.

### A.4 Mid-run reminder injection — rollout budget (the only mid-turn nudge in core)

`core/src/rollout_budget.rs` + `core/src/session/rollout_budget.rs` + `core/src/session/turn.rs:325-330`:
- Config `RolloutBudgetConfig {limit_tokens, reminder_at_remaining_tokens: Vec<i64>, weights}`
  (`core/src/config/mod.rs:1222-1227`); validated: thresholds must be positive and below limit (`:2812-2828`).
- **Per sampling step**, before the request: `maybe_record_reminder` → `pending_reminder(thread_id, window_id)`
  (`rollout_budget.rs:67-91`): `reminder_index = count(thresholds where remaining <= threshold)`;
  suppressed if the last delivery for this thread+window has `reminder_index >= current`. So it fires
  **once per threshold crossing**, monotone, never repeats at the same level.
- Delivered as a developer-role `<rollout_budget>` fragment: "You have {n} weighted tokens left in
  the shared session token budget." (`core/src/context/rollout_budget.rs:9-27`).
- `mark_reminder_delivered` only **after** history insertion; comment `:99`: "cancellation before
  then should retry it." Re-armed (`rearm_reminder`, `:112-118`) on rollback/compaction
  (`session/handlers.rs:338-341`) so a new context window restates the remainder.
- Hard stop: `record_usage` returns true at limit → `CodexErr::SessionBudgetExceeded` (`session/rollout_budget.rs:26-36`).

Also present but separate: `DEFAULT_TOKEN_BUDGET_REMINDER_MESSAGE_TEMPLATE` (`config/mod.rs:1109-1112`,
context-window-nearly-exhausted reminder, 2000-byte caps on custom text `:1113-1115`).

### A.5 `tool_choice`

`core/src/client.rs:951` hard-codes `tool_choice: "auto"` for every request; `:318-351` only
compares previous vs current for cache-key stability. Guardian scorer uses `"none"`
(`ext/guardian-v2/src/async_scorer/sampler.rs:481`). **No forced tool call anywhere** — codex
never forces `update_plan` or `update_goal`.

### A.6 codex failure-mode evidence (comments only; no changelog locally)

- Compaction-error continuation loop → block goal on any non-usage turn error (`extension.rs:311-314`).
- External goal mutation racing idle continuation → semaphore + deferral row (`runtime.rs:256-258,367-369`).
- Automatic wakeups entering Plan mode → refused at submission (`turn_input.rs:272-276`).
- Reminder lost on cancel → mark delivered after insert (`rollout_budget.rs:99`).
- Prompt drift: three inconsistent phrasings of the in_progress rule across 5 prompt copies (A.1).

---

## B. jcode (`jcode/`)

### B.1 `todo` tool — stateful, file-backed, self-assessing

Handler `crates/jcode-app-core/src/tool/todo.rs` (`TodoTool`, name `"todo"` `:721`), state
helpers `crates/jcode-base/src/todo.rs`, types `crates/jcode-task-types/src/lib.rs:433+`.

**Schema** (`tool/todo.rs:733-859`): `todos[] {content, status: pending|in_progress|completed|cancelled,
priority, id, group?, confidence: speculative|plausible|validated|verified, completion_confidence?}`;
`plan {user_intention, understands_user_intent: uncertain|partial|clear|complete}`;
`goals[] {group?, closed_feedback_loop, feedback_loop, feedback_loop_relevance, _coverage,
_traceability, delivery_state?, difficulty?, autonomy?, iteration_maturity?, stopping_evidence?}`.
Description is one line by design — comment `:726-730`: "SECURITY/EVAL: This is model-visible
calibration text ... Never generate it from gate constants or interpolate private thresholds,
because that would teach the model how to target the evaluator."

**Semantics:**
- Read = no `todos/goals/plan` keys (`:862-864`). Write **replaces** the todo list wholesale;
  goals/plan-only writes keep the stored list (`:866-868`).
- No "one in_progress" rule at all — status is free within the enum. Unknown status → hard
  error naming the vocabulary (`:75-90`; changelog v0.75.2 "rejects unknown status values instead
  of storing them silently"; v0.75.1 "Todo completion synonyms such as done and finished no
  longer trigger false auto-poke loops" → `canonical_todo_status` `jcode-base/src/todo.rs:42`
  tolerates legacy synonyms on load).
- Lenient input normalization (`:619-709`): stringified arrays/objects, `"90"` numbers, `""`→null
  (issues #357, #106 — Claude tool-call quirk).
- **Tool-owned histories**: `confidence_history` per todo (`:33-66`) and per-goal score histories
  (`:226-328`) — "one write contributes at most one observation so a single completion update
  cannot manufacture an apparent intermediate step"; model-supplied history ignored.
- Field-level merge for goals/plan (`:264-296`, `:396-412`) — a partial write must not erase
  other assessments, else "the turn-end digest would read a stale None and re-raise a point the
  agent had already resolved."
- `prune_orphaned_goals` (`:322-342`): goals whose group has no todo are dropped when the list is
  replaced (issue #695: stale goals shown indefinitely).
- Output (`:566-617`): pretty JSON of todos (+ plan/goals) with title `"{remaining} todos"`;
  assessment-only writes render only the changed fields (`:906-912`) "instead of repeating an
  otherwise identical todo plan".
- Publishes `BusEvent::TodoUpdated` (`:925-928`) → TUI `local.rs:291-296` refreshes widget + terminal title.

**Persistence** (`jcode-base/src/todo.rs:867-1097`): `~/.jcode/todos/{session}.json` (bare array),
`{session}-goals.json`, `{session}-plan.json`, `{session}-review-state.json`,
`{session}-gate-observations.json` (turn-scoped, cleared after digest, capped at
`MAX_GATE_OBSERVATIONS = 256` `:1091`). Survives restart/resume; drives session title
(`derive_session_title` `:967-1001`).

**Nesting**: one level — flat items with optional `group` label; goals are per-group. Also
`blocked_by`/`assigned_to` on `TodoItem` (used by swarm plan projection, `info_widget_todos.rs:17-31`).

**User editing**: none on content. `/todos` / Alt+X pins the card; v0.77.0 "expandable pinned
details ... activated with the mouse". `/poke on|off|status|trigger` controls continuation only
(`jcode-tui/src/tui/app/commands.rs:68-72`).

### B.2 TUI rendering

- Persistent info widget `crates/jcode-tui/src/tui/info_widget_todos.rs`: header with
  completed/total + pip meter (`push_todo_pips` `:208-283`: 1:1 pips below `EXACT_PIP_FLOOR=12`
  `:5`, else proportional with guaranteed ≥1 active/done pip); grouped sections, ungrouped last
  (`:339-341`); status sort in_progress→pending→completed (`:345-358`); "+N more" footer (`:477`);
  blocked marker `⊳` from `blocked_by` (`:405`); confidence colors (`:92-100`).
- Transcript delta card `ui_todo_changes.rs:1-60`: recovers previous list from the last `todo`
  write message (stateless, reload-safe, zero token cost `:4-8`); Form A one-liner for trivial
  change vs Form B block, `MAX_CHANGE_LINES = 6` (`:25`).
- Synthetic continuations are hidden: `is_auto_poke_message` (`jcode-base/src/todo.rs:760-801`,
  prefix match against every historical wording) and `auto_poke_display_summary` (`:803-865`)
  render a one-liner like "🔍 Reviewing the weak points of this turn for you..." instead of the
  model-facing text (changelog v0.36.0 "Auto-poke continuations no longer render as user prompts
  after reload"). Voice rules in `docs/MESSAGE_VOICE.md:10-48`.

### B.3 Mid-run nudges — deliberately deferred

`tool/todo.rs:462-563` `record_reframe_observations`, comment `:465-481`:
> "Previously both checks emitted a continuation on every applicable write for as long as the
> score stayed low. That punished the common healthy case: understanding of a request starts low
> and rises as the agent explores, so an agent already resolving the ambiguity was repeatedly told
> to stop and go resolve the ambiguity. On long iterative turns the same text reattached to every
> todo call, spending reasoning on re-justifying the plan instead of on the work. So the checks are
> deferred: observations accumulate and are replayed once at turn end ... Deferred, not forgiven."

Rules:
- Every write with an open todo whose plan/goal assessment fails its bar → append a
  `GateObservation {kind, group, state}` to the per-session file (`:864-880`); never returned to
  the model, except:
- **One immediate nudge**: first plan write (`understands_user_intent_history.len() <= 1`) scoring
  `<= SEVERE_INTENT_MISUNDERSTANDING` (= `Uncertain`, `jcode-base/src/todo.rs:323`) appends
  `TODO_INTENT_UNDERSTANDING_CONTINUATION_MESSAGE` (`:177`: "[auto] Understand the user's intent
  better. Try to avoid asking the user. Make sure the todo is up to date.") to the tool result
  (`tool/todo.rs:495-506`). Rationale `:319-322`: "a whole turn spent on the wrong task cannot be
  recovered at turn end."
- Groups closed *by this write* are also observed (`:509-521`) — "one-step completions are where
  a weak feedback loop hides best."
- Static gate text budget: `TODO_QUALITY_GATE_MAX_APPROX_TOKENS = 64` (`jcode-base/src/todo.rs:16`)
  "short enough to be read as a nudge, not a replacement system prompt"; named-todo lists in
  continuations capped at `GATE_NAMED_TODO_LIMIT = 6` with "(and N more)" (`:675,687-706`), labels
  truncated at 80 chars (`:677-685`).
- Changelog: v0.61.0 "Quality-gate reminders about open todos now arrive once at the end of a
  turn instead of nagging after every write"; v0.78.0 "shorter, clearer guidance and avoid
  repeatedly blocking final responses"; v0.70.0 "more targeted and neutral prompts".

### B.4 Stop-time interception — the auto-poke state machine (TUI)

Entry `crates/jcode-tui/src/tui/app/input.rs:1540-1548` `schedule_turn_end_followups`, run at
every turn end: guardrail circuit breaker first, then `schedule_auto_poke_followup_if_needed`
(`:1597-1782`), then overnight. Continuations are pushed to `queued_messages` as **user-role**
messages (`:1701-1703` comment: "reminder-only turns read as empty user messages and models answer
instead of re-validating"). Default on: `features.auto_poke = true`
(`jcode-base/src/config/default_file.rs:285`), env `JCODE_AUTO_POKE`.

Decision order, exact:
1. Bail if disarmed, dispatch pending, turn pending, or anything already queued (`:1598-1604`).
2. **Long-session review** (`:1611-1621`): if todos exist and `take_long_session_review_if_due`
   → queue `TODO_LONG_SESSION_REVIEW_MESSAGE` ("[auto] Re-read the request. Update the todo plan
   and goal assessments from the evidence gathered so far..."). Fires **once per todo cycle**
   after `TODO_LONG_SESSION_REVIEW_AFTER = 30 min` (`jcode-base/src/todo.rs:20`, "Private
   policy. Do not include this duration in model-facing schemas"); cycle = clock started when a
   fully-completed list is replaced by open work (`update_todo_review_cycle` `:905-932`);
   `review_delivered` latched before queueing "so reloads cannot duplicate it" (`:935-951`).
3. **Incomplete todos exist** (`:1758-1782`): fingerprint = JSON of incomplete items
   (`:1751-1752`); if equal to `last_auto_poke_fingerprint` → **idle, no poke** ("unchanged_todos",
   `:1753-1758`; changelog v0.69.0 "avoid repeated unchanged prompts"). Else queue
   `build_auto_poke_message(n)` = "You have N incomplete todo(s). Continue working, or update the
   todo tool." (`jcode-base/src/todo.rs:665-671`), show "👉 N incomplete todos. We poked it for
   you. /poke off to stop.", reset gate attempts to 0 (`:1778` "Open todos mean the model is still
   iterating"), reset final-response flag and spike latch.
4. **All done, none exist** (`:1636-1642`): stay armed, do nothing ("disarming here would
   silently kill the feature for the whole session after the very first todo-free turn").
5. **All done** — ordered gates, each queues exactly one continuation and returns:
   a. Gate digest (`deliver_deferred_gate_digest_if_needed` `:1554-1595`): once per turn
      (`todo_gate_digest_delivered`), builds `build_gate_digest` (`jcode-base/src/todo.rs:416-537`)
      — repeats of the same (kind, group) collapse to one line "(flagged N times this turn)"
      (`:428-436`, `:527-531`); points whose score later cleared are still raised but reworded as
      "run it over the earlier work" (`:373-405`); prefix "[auto] Before you treat this turn as
      finished, double-check the weak points it surfaced..." (`:363`). Observation file cleared
      either way "so the next turn starts clean."
   b. Ownership gate (`:1650-1670`): `!completed_groups_have_sufficient_delivery` and attempts <
      `TODO_COMPLETION_GATE_MAX_ATTEMPTS = 5` (`app.rs:1708`) → `build_todo_ownership_continuation_message`
      (per-goal bullets naming *what* to do, "without exposing fields, scores, thresholds, or
      pass/fail language" `jcode-base/src/todo.rs:204-206`).
   c. Completion-confidence / spike gate (`:1671-1706`): missing or sub-threshold
      `completion_confidence`, or a spike (`TODO_CONFIDENCE_SPIKE_LEVELS = 2` levels in the last
      history step, `jcode-base/src/todo.rs:317,644-662`) not yet challenged
      (`todo_confidence_spike_challenged` latch, once per cycle, survives reload per v0.49.0).
   d. **Exhaustion** (`:1707-1728`): gate still failing and attempts ≥ 5 → disarm auto-poke,
      reset all latches, show "⚠️ We nudged the agent several times but its validation still
      isn't holding up. We stopped poking; review the remaining todos yourself." Comment: "Nudging
      again would loop forever, burning an API call per turn (observed live: an unattended session
      resent the same continuation every ~5s)." Constant comment `app.rs:1704-1708` same incident.
   e. **Clean finish** (`:1729-1747`): re-arm to `auto_poke_default_on`, clear digest/attempt
      latches, and queue **one** `TODO_FINAL_RESPONSE_CONTINUATION_MESSAGE` ("[auto] Quality
      checks passed. Give the user a concise final response now. Do not call the todo tool or do
      more work." `jcode-base/src/todo.rs:310`, rationale `:307-309`: "Gate continuations tell the
      model not to reply, so without this handoff a cycle can end on a bare tool call or an
      internal-looking validation response."), guarded by `todo_final_response_requested`.

**Circuit breakers around the whole thing:**
- Guardrail refusals: `GUARDRAIL_STOP_MAX_CONSECUTIVE = 2` (`app.rs:1714`), counted per finished
  turn (`input.rs:1510-1516`); on trip, disarm auto-poke and overnight
  (`stop_auto_continuation_after_guardrail` `:1521-1536`, `disable_auto_poke` `commands.rs:106-118`
  also sets `auto_poke_default_on = false` so the default re-arm cannot resurrect it). Comment
  `:1506-1509`: "a guardrail refusal is deterministic for the same request, so re-poking loops
  forever (observed live as one refused API call per auto-poke, every ~7s)." Changelog v0.52.0.
- Credential failures: consecutive-401 breaker (`app.rs:1715-1720`, "18k in one session").
- Overnight mode (`commands_overnight.rs:11-13,313-400`): `OVERNIGHT_STALL_LIMIT = 3` no-progress
  turns (progress = fingerprint change), `OVERNIGHT_ERROR_LIMIT = 2`, poke budget
  `clamp(4 * ceil(hours), 4, OVERNIGHT_MAX_POKES = 48)` (`:531-539`).

**Headless `jcode run`** (`src/cli/commands.rs:2498-2827`): same three follow-ups
(Incomplete → GateDigest → ConfidenceSummary, `:2669-2703`), loop bounded only by optional
`JCODE_RUN_AUTO_POKE_MAX_TURNS` (`:2593-2604`) — **no fingerprint check**, so an agent that stops
without touching todos re-pokes until the cap or the model marks them done. Digest is taken only
once no work remains (`:2653-2666`: taking it earlier "would destroy the reminder"). Confidence
summary is skipped when nothing is actionable (`:2732-2736`: "spends tokens on 'all good' theater").

### B.5 Mission mode (jcode's analogue of codex goals)

`crates/jcode-app-core/src/mission.rs` + `jcode-base/src/prompt/mission_continuation.md` (58 lines,
clearly derived from codex `continuation.md`, plus `<long_horizon_intent>`, "three layers"
interpretation `:16`, "Continuously refresh the todo frontier" `:17`, and a verification menu
`:34-37`). Attached as a per-turn reminder to *every* user prompt while active
(`input.rs:69-74` `mission_turn_reminder`, `active_system_reminder` `mission.rs:143-151`) — not an
idle-triggered auto-turn; the auto-poke machine above supplies the "keep going".
Blocked audit `:49-52` drops codex's "three consecutive turns" count: "Do not stop the first time
a blocker appears."

### B.6 jcode failure modes, cited

| Incident | Bound applied | Cite |
|---|---|---|
| Same gate continuation resent every ~5s indefinitely | `TODO_COMPLETION_GATE_MAX_ATTEMPTS = 5`, reset on open-todo progress | `app.rs:1704-1708`, `input.rs:1707-1728` |
| Refusal + auto-poke alternating every ~7s | 2 consecutive guardrail stops → disarm, sticky | `input.rs:1506-1516`, v0.52.0 |
| 18k 401s in one session | credential breaker | `app.rs:1715-1720` |
| Nag on every todo write while score rises | defer to turn-end digest; one severe-first exception | `tool/todo.rs:465-481`, v0.61.0 |
| Wall of duplicates on long turns | collapse (kind,group) with count; 256-obs cap; 6 named todos | `jcode-base/src/todo.rs:410-412,675,1091` |
| Poke with unchanged todos = stall, not progress | fingerprint of incomplete items | `input.rs:1751-1758`, v0.69.0 |
| "done"/"finished" statuses → false incomplete loop | strict enum + canonicalizer | v0.75.1/v0.75.2, `tool/todo.rs:75-90` |
| Stale goals shown forever after list replaced | prune orphaned goals | `tool/todo.rs:322-342` (#695) |
| Cycle ends on bare tool call / internal-looking reply | one final-response handoff turn | `jcode-base/src/todo.rs:307-311` |
| Continuations re-rendered as user prompts on reload | prefix classifier + display summary | `jcode-base/src/todo.rs:760-865`, v0.36.0 |
| Model targets the evaluator | thresholds/timers never in model-facing text | `tool/todo.rs:726-730`, `jcode-base/src/todo.rs:6-8,18-20` |
| Auto-poke silently never dispatched | `AUTO_POKE_DECISION` log line on every arm | `input.rs:1764-1777` |
| Digest consumed while work still open | take only when no work remains | `src/cli/commands.rs:2653-2666` |

---

## Lessons for a todo tool with mid-run nudges and stop-time interception

1. **Validate state rules in the handler, not the prompt.** codex says "exactly one in_progress"
   in prompts three inconsistent ways and enforces none of it (`plan.rs:90`, `plan_spec.rs:46`,
   `default.md:273`, `gpt_5_2_prompt.md:46`). jcode enforces status vocabulary and got a
   real bug class out of not doing so earlier (v0.75.1/.2). Reject in the tool result with the
   exact vocabulary.
2. **Tool result text is the nudge channel; keep it constant when nothing changed.** codex returns
   the literal `"Plan updated"` (`plan.rs:22`) and tells the model not to echo the plan
   (`default.md:58`). jcode renders only the changed fields on assessment-only writes
   (`tool/todo.rs:906-912`). Both avoid transcript spam.
3. **Mid-run: defer, don't interrupt.** jcode's write-time gates nagged healthy agents whose scores
   were rising; they moved everything to a turn-end digest with one exception (severe
   first-plan misunderstanding), collapsed repeats with a count, and capped the log at 256
   (`tool/todo.rs:465-481`, `jcode-base/src/todo.rs:410-436,1091`). If Yi nudges mid-run at all,
   it should be a threshold-crossing event delivered once per crossing, like codex's rollout budget
   (`rollout_budget.rs:76-86`: index = thresholds crossed, suppress if already delivered ≥ index;
   mark delivered only after the message is in history; re-arm on compaction).
4. **Stop-time interception needs four bounds, all mechanical:** (a) attempt cap per cycle
   (jcode 5, reset when open todos change); (b) fingerprint of the open set — identical set →
   don't poke (jcode TUI has it, `jcode run` does not and relies on an env cap); (c) breaker on
   deterministic provider errors (2 consecutive refusals, sticky disarm that beats the default
   re-arm); (d) a terminal state the model can only reach through a status enum
   (codex `update_goal` accepts `complete|blocked` only, `tool.rs:232-240`; anything else is the
   system's call). codex additionally blocks the goal on any non-usage turn error to stop
   compaction-error loops (`extension.rs:311-314`).
5. **Order stop-time follow-ups and send exactly one per turn end.** jcode: long-session review →
   incomplete-poke → digest → ownership → confidence/spike → exhaustion → final-response handoff,
   each returning after queueing (`input.rs:1597-1782`). The final-response turn exists because
   "do not reply" continuations otherwise leave the user with a bare tool call
   (`jcode-base/src/todo.rs:307-311`).
6. **Continuation messages: user-role for the model, hidden from the user.** jcode sends them as
   user messages (empty-looking reminder turns get answered instead of acted on,
   `input.rs:1701-1703`) but classifies them by prefix and renders a one-line notice
   (`todo.rs:760-865`). codex uses a developer-role tagged fragment (`steering.rs:49-54`) and never
   shows it. Either way: a stable `[auto]`-style prefix that survives reload is load-bearing.
7. **Never expose thresholds, timers, or evaluator language to the model.** jcode keeps the
   30-min review timer and every gate bar private (`todo.rs:6-8,18-20`; `tool/todo.rs:726-730`);
   continuations name the category and the target todo, not the score.
8. **Persist the list; treat the event as transient.** codex persists nothing (the checklist is
   rebuilt from the function-call history and vanishes from the status line on resume,
   `policy.rs:168`); jcode persists per-session JSON files and derives session titles from them
   (`todo.rs:867-1001`). Turn-scoped state (observations, digest-delivered, attempt count) must be
   cleared at cycle boundaries or a session can only ever deliver one digest (`input.rs:1731-1735`).
9. **Cap everything model-facing by size, not by trust:** 64-token static gate text, 6 named
   todos, 80-char labels, 6 delta lines in the transcript card, 256 observations
   (`jcode-base/src/todo.rs:16,675-706`, `ui_todo_changes.rs:25`).
10. **Guard automatic turns against mode and concurrency.** codex refuses idle auto-starts in Plan
    mode, when a turn is active, or when trigger mail is pending (`turn_input.rs:266-300`), holds a
    semaphore across read-then-start (`runtime.rs:367-369`), and defers continuation after any
    external goal edit until a real turn starts (`goals.rs:109-117`, `extension.rs:205-212`).
11. **Log the decision.** jcode's `AUTO_POKE_DECISION action=... reason=...` line exists because a
    queued-but-never-dispatched poke was indistinguishable from a silent model (`input.rs:1764-1777`).
12. **Nesting/editing:** neither reference supports nested items or user edits of the checklist;
    jcode's one-level `group` + per-group goals is the most structure either needed. codex keeps
    the checklist strictly separate from the user-editable goal (`/goal edit`) and from Plan mode.
