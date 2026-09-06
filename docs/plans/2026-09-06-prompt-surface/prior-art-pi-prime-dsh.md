# Prior art: task tracking and "keep working until done" in Pi, prime-agent, deepseek-harness

Read-only survey, 2026-09-06. Paths are relative to `ref/agents/<repo>/`. Appendix-A exclusions were honored (no pi test/tui/modes/examples; no dsh docs/apps/typert/api-catalog/api-proxy; no test dirs anywhere). Line numbers are from the 2026-08 clones.

---

## A. Pi (`pi/packages/agent/src`, `pi/packages/coding-agent/src`)

### A.1 No todo/task/plan tool in core

- `grep -rniE todo` over `packages/agent/src` and `packages/coding-agent/src` hits only compaction/export code. There is no built-in todo, plan, or task tool. `docs/extensions.md:2927` lists `todo.ts` ("Stateful tool with persistence") and `:2975` `plan-mode/` as *example extensions* under `packages/coding-agent/examples/` (excluded, not opened). The core contract is: a todo list is extension-owned state persisted via `appendEntry` (docs `:1946-1948` show the `promptSnippet`/`promptGuidelines` shape a todo tool advertises).
- Consequence: Pi has no stop-time interception keyed on task state. Its "continuation" machinery is generic queues + hooks.

### A.2 Loop shape and the four continuation seams

`packages/agent/src/agent-loop.ts:155-266` (`runLoop`):
- Outer `while(true)`; inner `while (hasMoreToolCalls || pendingMessages.length > 0)` (`:173`).
- `:196-200` — `stopReason === "error" | "aborted"` ends the run unconditionally (`agent_end`, return). No continuation hook is consulted.
- `:208-215` — `stopReason === "length"` with tool calls: every tool call is failed, none executed (`failToolCallsFromTruncatedMessage`, `:373-407`). Comment `:374-379`: the streamed-argument JSON salvage parser can produce calls that "parse and validate but are silently incomplete. None of them are safe to execute; report each as an error so the model can re-issue them." Error text `:394-396`. `terminate: false` (`:406`) so the loop continues to let the model re-issue.
- `:239-249` — `config.shouldStopAfterTurn?.(...)` true → `agent_end`. Host-owned stop veto.
- `:251` — `getSteeringMessages()` polled after each turn's tool results (mid-run injection).
- `:254-263` — when no tool calls and no steering: `getFollowUpMessages()`; non-empty → another turn. This is the only "agent would stop, keep going" seam in the core loop.
- `types.ts:222-257` documents both: steering "Called after the current assistant turn finishes executing its tool calls"; follow-up "Called when the agent has no more tool calls and no steering messages… Contract: must not throw or reject."
- `types.ts:68,94,374` — `terminate?: boolean` on tool results: a tool can end the batch early (`beforeToolCall` block may also set it, `:274`).

`packages/agent/src/agent.ts`:
- `:231-232` — both queues default to `"one-at-a-time"` drain mode (`PendingMessageQueue`), i.e. one queued message per turn boundary, not a flood.
- `:283-310` — `steer()`, `followUp()`, `hasQueuedMessages()`.
- `:361-388` — `continue()` refuses when last message is assistant unless a steering/follow-up message is queued; otherwise `runContinuation()` (last message must be user/toolResult, `agent-loop.ts:57-76`).

### A.3 Coding-agent post-run loop (retry, compaction, queued extension messages)

`packages/coding-agent/src/core/agent-session.ts`:
- `:1074-1085` `_runAgentPrompt`: `await agent.prompt(); while (await _handlePostAgentRun()) await agent.continue();` then `_emitAgentSettled()`.
- `:1087-1116` `_handlePostAgentRun`: order is (1) retryable error → `_prepareRetry`; (2) `auto_retry_end` failure emit; (3) `_checkCompaction`; (4) `return this.agent.hasQueuedMessages()` with comment `:1113-1114`: "The agent loop drains both queues before emitting agent_end. Any messages here were queued by agent_end extension handlers and need a continuation." → an extension's `agent_end` handler calling `sendMessage/sendUserMessage` is Pi's supported "keep working" hook.
- Retry cap: `:2807-2855` `_prepareRetry` — `_retryAttempt++`; `> settings.maxRetries` → decrement and stop; delay `baseDelayMs * 2 ** (attempt-1)`; abortable sleep. Defaults `settings-manager.ts:878-883`: `maxRetries: 3`, `baseDelayMs: 2000` (2s/4s/8s).
- Overflow compaction retry `:2300-2320`: after compaction `willRetry` strips a trailing assistant error/length message ("agent.continue() rejects that state", `:2308-2312`) and returns true; otherwise returns `hasQueuedMessages()` (`:2318-2320`) — "Auto-compaction can complete while follow-up/steering/custom messages are waiting. Continue once so queued messages are delivered."
- `:1448-1480` `sendCustomMessage`: `deliverAs: "steer" | "followUp" | "nextTurn"`. `nextTurn` (`:1461-1462`) parks the message in `_pendingNextTurnMessages` and it rides alongside the *next user prompt* (`:1237-1241`) — never triggers a turn. Streaming + `triggerTurn !== false` → queue; idle + `triggerTurn` → `_runAgentPrompt`.
- `extensions/types.ts:734-747`: `agent_end` vs `agent_settled` — "Fired after an agent run has fully settled and no automatic retry, compaction, or queued continuation will run." `docs/extensions.md:569` repeats this (the reason the event exists: status integrations were firing on `agent_end` while Pi was still going to continue).

### A.4 Forced tool choice, reminder injection

- No `toolChoice`/forced tool call anywhere in `packages/agent/src` or `packages/coding-agent/src/core` (grep). Pi never forces a tool.
- No system-reminder injection in core. Extension-side injection is `sendMessage` with `customType` (a `role: "custom"` message, `:1452-1460`), rendered/persisted as its own entry.

### A.5 How tasks show to the user

Not applicable in core (no task model). Custom messages carry `display?: boolean` (`:1457`) so a todo extension chooses whether its state message renders. Events available to a UI: `turn_start/turn_end` (with `turnIndex`, `toolResults`), `message_*`, `tool_execution_*`, `agent_end`, `agent_settled` (`extensions/types.ts:716-770`).

### A.6 Failure-mode evidence

- Truncated-arguments incident (`agent-loop.ts:374-379`) — executing salvaged JSON from a `length` stop; fix = fail all calls, continue.
- `agent_end` not being final (docs `:569`; `agent_settled` added).
- Stale trailing assistant message breaking `continue()` after overflow compaction (`:2308-2312`).
- Bash results during streaming would "break tool_use/tool_result ordering" → deferred to `agent_end` flush (`:2951-2956`).
- Git history unavailable (shallow clone, 1 commit).

---

## B. prime-agent (`prime-agent/packages/agent/src`, `packages/coding-agent/src`, `prime-agent-runtime/`)

### B.1 No todo tool; the unit is the *goal*

- `grep -niE 'todo|task|plan' packages/coding-agent/src/core/tools/index.ts` → nothing. `prime-agent-runtime/src/rlm/harness.py:705-720` has `plan_refinement` (harness self-improvement), not a task list. Task tracking is a single durable **goal** per thread plus an **autonomous mode** policy.

### B.2 Loop: a third seam, `getContinuationMessages`, ranked last

`packages/agent/src/agent-loop.ts:303-447` (Pi's loop plus):
- `:316` `shouldStopBeforeTurn()` (host: pending steering-stop action).
- `:365-386` `shouldStopAfterTurn` checked; `:393-405` steering; `:415-424` follow-ups; `:429-441` **`getContinuationMessages(lastTurn, signal)`** only when follow-ups were empty. `types.ts:236-243`: "Use this for host-owned continuation policies such as long-running goals. Explicit follow-up messages always take precedence over continuation messages. Contract: must not throw or reject."
- Precedence rule (explicit): human/queued work > automatic continuation.

### B.3 Goal state machine

`packages/coding-agent/src/core/goals.ts`:
- `:8` `MAX_THREAD_GOAL_OBJECTIVE_CHARS = 4000`.
- `:10` `GoalStatus = idle | active | paused | budget_limited | complete | error`; `:11` context kinds `continuation | budget_limit | objective_updated`.
- `:13-26` state: `tokenBudget?`, `tokensUsed`, `timeUsedSeconds`, `continuationsUsed`, `lastReason`, `lastError`. Persisted as custom entry `thread_goal_state` (`:4`).
- `:154-180` `createGoalContextMessage` — a `role: "custom"`, `customType: "goal_context"`, `display: true` message wrapping `<goal_context>…</goal_context>`. This is the nudge, and the user sees it (display true).
- `:207-229` continuation prompt: "Continue working toward the active thread goal… The goal persists across turns. Ending one turn does not reduce or redefine the objective… Before marking the goal complete, audit the current state against every requirement… Do not call `goal.complete()` unless the goal is complete. Do not mark a goal complete merely because the budget is nearly exhausted or because you are stopping work." Objective is XML-escaped and labeled untrusted (`:214`, `:288-290`).
- `:232-250` budget_limit prompt: "Do not start new substantive work. Wrap up this turn soon with progress made, remaining work, blockers, and a concrete next step."
- Completion is model-invoked from the kernel: `skills/goal/src/goal/__init__.py:44-52` `await goal.complete()` → host request `goal.complete`. `:25-33` `create` "Fails while a goal is still pending… Only create a goal when the user … long-running goal."

### B.4 Goal continuation driver (agent-session.ts)

- Seeding `:1274-1284`: `--goal` only on a fresh, top-level, bootstrap-only branch ("prevents reseeding after clear/complete/error or restart"). Context rides `_pendingNextTurnMessages` (`:1283`).
- `:2034-2054` `_ensureGoalRuntimeActive` force-adds the `ipython` tool to the active set and to the live continuation context "so the model can always reach `goal.complete()`" (the only forced-tool behavior found: forced *availability*, not forced *choice*).
- `:3273-3304` `_getGoalContinuationMessages`: returns [] when terminal (`error|aborted`, `:1879-1889` also finishes the goal), not active, or **children unsettled** (`:3284-3288`: "Delegating and ending the turn is correct behavior; hold the continuation until descendants settle instead of re-prompting a waiting parent" → `_goalContinuationAwaitsRlmWork = true`, resumed at `:2057-2088`). Otherwise `continuationsUsed + 1` and one `continuation` message.
- `:3311-3346` `_getContinuationMessages`: `if (this.queuedActionCount > 0) return []` (human work wins); snapshot goal state, and if `_sessionInputArrivalEpoch` changed while awaiting, **roll back** the increment and return [] (`:3322-3326`). Autonomous continuation runs only after goal yields nothing (`:3338-3345`) with the same epoch rollback.
- Budget accounting `:2141-2177`: per assistant `message_end`, only while `status === "active"`, tokens = input+output (`goals.ts:96-98`); reaching budget flips to `budget_limited` and `_shouldStopAfterTurn` (`:2199-2211`) queues the `budget_limit` context **as steer** (so it lands before the next LLM call of the same run). Comment `:2151-2156`: usage attributed at message_end "before that turn's ipython cell runs. goal.complete() only arrives later… so the completing turn is always accounted while the goal is still active."
- Threshold-compaction interplay `:2828-2846` and rollback `:2852-2866`: a continuation queued for compaction that the user cancels has its `continuationsUsed` decremented "so the next natural stop re-queues it"; dedupe check `:2815-2826` (only undelivered actions deduplicate).
- **No cap on goal continuations by count.** The only bounds are: explicit `tokenBudget`, terminal stop reasons, `goal.complete()`, `/goal clear|pause`. Wall-clock is tracked, not enforced.
- UI: `formatGoalUsage` (`goals.ts:182-190`) "N / budget tokens" or "Ns"; `/goal status` (`:1962` renders autonomous status string).

### B.5 Autonomous mode (bounded host policy)

`packages/coding-agent/src/core/autonomous.ts`:
- `:45-46` continuation prompt: "No human input is available in autonomous mode. Continue working until the host evaluator, verifier, or configured autonomous limits stop the run… Do not end the session yourself; the verifier/evaluator decides completion when configured gates pass."
- `:50-56` defaults: `maxContinuations: 3`, `maxTurns: 12`, `maxTokens: 80_000`, `timeoutMs: 30 min`. `:58-62` gates: `maxRetries: 3`, `timeoutMs: 5 min`. `:64` `MAX_GATE_OUTPUT_CHARS = 6000`.
- `:183-189` token delta = `input + output + cacheWrite` with comment: "Cache-read tokens are repeated context served from provider cache. Counting them cumulatively makes long autonomous verifier loops exhaust their host-side token budget far before the non-cached work reaches the configured cap." (an incident: budget exhausted by cache reads).
- `:221-247` decision: `error|aborted` → no; gates pass → no (`not_needed`); gate `retry_exhausted` or limit → `limit_reached`; gate failed → continue with gate output; no gates → continue with reason `missing_terminal_evidence`.
- `:249-267` `autonomousLimitReason`: continuations ≥ max, turns ≥ max, tokens ≥ max, elapsed ≥ timeout.
- `:283-303` **unchanged-workspace guard**: if the same gate failed and the git worktree snapshot (status+diff+untracked hash) is identical, the gate is *not rerun*; the attempt counter still increments and the model gets: "The autonomous gate was not rerun because the workspace has not changed since this failure. Edit source files, tests, or a blocker artifact before attempting to finish again." → `retry_exhausted` after `maxRetries`.
- `docs/long-running-agents.md:201,221,239`: "bounded host policy… follow-up continuations until configured quality gates pass or a continuation, turn, token, or wall-clock limit is reached"; "Compaction is not a completion signal."

### B.6 Heartbeats (scheduled nudges) and how they are bounded

`packages/coding-agent/src/core/cron-jobs.ts`:
- `:18` sources `cron | heartbeat | rlm_heartbeat`; `:117-118` `DEFAULT_HEARTBEAT_SCHEDULE = "every 5m"`, default delivery `"steer"`; `:24-27` steer interrupts current turn, `follow_up` waits.
- One user heartbeat per session (`:313-331` `createHeartbeat` rejects duplicates and non-recurring schedules); agents may create several `rlm_heartbeat`s (`:127-143`).
- Defer rule `:1350-1369`: deferred when compacting, retrying, bash running, pending session work, or idle-with-unfinished-actions; a plain streaming turn defers `follow_up` but not `steer`.
- Deferred = **skipped, not queued**: `recordSkipResult` `:672-696` advances `nextRunAt` to the next schedule tick and stamps `lastSkippedAt`. Claim coalescing `:1573-1598`: due ticks are claimed (dispatch record) before delivery; a job with an in-flight dispatch is advanced and marked skipped instead of double-dispatched. `docs/long-running-agents.md:170`: "Due ticks are claimed before delivery so a crash does not replay an uncertain prompt, and missed ticks are coalesced rather than accumulated into an unbounded backlog."
- Kernel prompt `prompts/rlm.ts:17`: "Do not keep the turn open by polling with `time.sleep()`… otherwise end the turn." — the model is told to *stop* and let heartbeats/continuation re-enter, rather than spin.

### B.7 Failure-mode evidence (comments)

- Reseeding a goal on restart (`:1276-1279`).
- Re-prompting a parent whose children are still running (`:3284-3286`).
- Continuation increment racing a human prompt (epoch rollback `:3322-3326`, `:3341-3344`).
- Compaction-cancel leaving a phantom continuation charge (`:2852-2866`).
- Cache-read tokens exhausting budgets (`autonomous.ts:184-187`).
- Rerunning an identical failing gate on an unchanged tree (`autonomous.ts:283-303`).
- `goal.complete()` accounted after the completing turn (`:2151-2156`).
- Git history unavailable (shallow clone).

---

## C. deepseek-harness (`deepseek-harness/packages/`)

### C.1 `todo_write` (`packages/todo/tool-todo/src/index.ts`)

- Whole-list replace, three statuses (`:26`, `:45-66`). Description: "Send the ENTIRE list every call — it REPLACES the previous list… add one todo per concrete step before you start… Mark a todo `completed` the moment it is done (do not batch completions), and allow no `in_progress` item only once all work is complete. Skip the list for trivial single-step tasks."
- Config `allowParallelInProgress: boolean` **required** (`:29-43`); false → "at most one task may be in_progress" enforced at execute (`:107-109`) with `isError` result so the model self-corrects. Validation `:91-111`: trimmed, non-empty, unique content.
- Persistence: `exec.agent.session.append('todo/write', { todos })` (`:213`); non-agent caller rejected (`:208-212`). Result text is counts only (`:201-204`).
- **Display lifetime** `:131-148` projection: latest `todo/write`; `turn/start` → `null`. Note `.agents/notes/implemented/feature/2026-07-28-todo-plan-clears-on-next-turn.md`: problem — "a completed or abandoned checklist from the previous task" stayed on screen; decision — `turn/end` keeps it visible, next `turn/start` clears; rejected alternatives: clear on turn/end (hides while user reads), clear only when all completed (leaves abandoned plans), synthetic empty write (mutates log for a UI rule).
- Invariant `invariant.ts:15-23`: durable rule validates shape/uniqueness only; the in_progress count is deliberately *not* a durable invariant "a log written while parallel work was allowed must still replay after a deployment tightens the policy."
- Model surface: `.agents/notes/…/2026-06-29-todo-write-tool.md:60` — "The event stays off the model surface, so a todo update never perturbs derived model history — the model sees only its own tool call and result."
- **No stop-time interception, no reminder keyed on todos.** `grep in_progress|todo/write` outside tool-todo hits only session types, session-query extraction (`extraction.ts:27-28` flattens status+content for search), and the experimental agent-team task board. The repeat-tool-reminder README shows `exclude: [todo_write]` as the canonical config so bookkeeping calls don't launder or reset loop detection.

### C.2 Goal domain (`packages/goal/goal/src`)

- `types.ts:44-48` durable phases `active | paused | blocked | complete`; `:71` process-local `activation: armed | disarmed` "never persisted"; `:19-24` CAS ref `{id, revision}`; `:67` `maxGoalRounds`; `:76` `roundsStarted` folded from the session log.
- `index.ts:187,196` `defaultMaxGoalRounds: 256`; `:321-325` resume refuses at cap: "exhausted N goal rounds; increase maxGoalRounds before resuming".
- Blocked carries `{code, message}` (`types.ts:51-56`); codes seen: `round-limit`, `queue-failed`, `prompt-rejected`, `model-reported`, `usage-limited`.

### C.3 Goal-round driver (`packages/goal/goal-round-driver/src/index.ts`) — the "keep going" mechanism

- Hierarchy Goal → Round → Turn → Step (note 2026-07-16 `:26`); only goal-sourced `user/message`s count as rounds; human turns never do.
- `readyToDrive` `:103-109`: fiber active, not stopping, exact live agent, `status === 'idle'`, `!competingQueued`.
- `drive` `:138-205`: durability checkpoint first (`sessions.flush`, `:142-154`; failure → disarm); if a previous attempt exists it is settled (`:156-162`); goal must be `active` + `armed` (`:165`); **`roundsStarted >= maxGoalRounds` → `block(round-limit)`** (`:166-172`); reserve identity `{goalId, revision, round}` + rendered content, then `agent.followup(message)` (`:174-192`); queue failure → `block(queue-failed)`.
- At most one reservation per agent; triggers coalesce into one serialized driver task (`:207-241`).
- Race fences: competing inbox insert marks the attempt stale (`:284-291`); `agent/pre-step` (`:349-414`) validates the reservation **before and after** downstream hooks (`validReservation` `:333-347`: exact id/revision/round/content, still armed, `round === roundsStarted + 1`) and rejects otherwise, restoring other claimed messages (`:126-135`).
- Terminal handling `:307-331`: `turn/end` with `max-tokens` → **disarm** (no automatic retry); `aborted` → attempt cancelled → on next idle the goal is **paused** (`:259-277`); `agent/error` → disarm (`:246-249`). Plugin load over existing agents disarms everything (`:416-421`: "never inherits hidden automatic authority").
- Prompt `prompt.ts:12-26`: `<goal_round>` with JSON-quoted objective and `Round: r/max`; "Treat the current workspace, tool results, and durable session state as authoritative; inspect them instead of assuming earlier narration is still current… If work remains, leave the goal active for the next round. Follow the configured goal-tool policy before reporting a blocker."
- Invariant companion `invariant.ts:45-58`: every goal-round message in the log must byte-equal the renderer's output for the folded prior state (so a forged/duplicated nudge fails the invariant).
- Design note 2026-07-19 goal-round-driver: `:11` the naive `goal/changed -> agent.followup()` listener "can admit obsolete work, run alongside a human prompt, spend beyond the cap, or restart from replay without new authority"; `:41-50` outcome table (completed → continue while armed & under cap; cancel → pause+disarm; RATE_LIMIT/QUOTA → block usage-limited; checkpoint failure → disarm) and "No abnormal outcome requests an automatic retry"; `:80` persisting a reservation rejected because "only the durable `user/message` consumes the round"; `:83` counting every turn as a round rejected because "human clarification and unrelated work share the session but not the automatic-work budget"; `:98` "`maxGoalRounds` is only an admitted-round limit. Token, currency, wall-clock… require independent policy."

### C.4 Goal tools (`packages/goal/tool-goal/src`)

- `index.ts:33` `blockedAfterConsecutiveRounds` default 3; `:299-306` `blocked` from a goal round is rejected before that many admitted rounds (`GOAL_TOOL_BLOCK_THRESHOLD`). Guidance `:113-123`: "Mark complete only when the objective is actually achieved. Mark blocked only after the same blocking condition persists for at least N consecutive goal rounds… difficulty, uncertainty, or useful remaining work is not blocked."
- Authority `authority.ts:70-83,101-108`: create/edit/pause/resume need a `{kind:'user'}` message in the current root-agent turn; complete/blocked need that or the *exact* current goal round. Note 2026-07-19 goal-tools `:45`: "Rely on prompt instructions for authority — rejected because text can guide model judgment but cannot authenticate the live caller, turn, or source event."
- **Stop-time wrap-up** `index.ts:313-325` + `wrapup.ts:17-41`: on autonomous `complete|blocked` the tool `deferContext`s a `<goal_complete>`/`<goal_blocked>` user message: "Write the closing message to the user now… Do not call any more tools in this run." Bug-fix note 2026-08-02: the original design called `concludeTurn()` at the tool result, "Sessions ended on a bare `update_goal` card, and internal testers read that as the agent stopping mid-sentence"; the fix costs "one additional model request per goal lifecycle, not per round"; wording A/B'd — a structured instruction beat "summarize", and the no-instruction control "produced high-variance closings, including confidently fabricated file-level detail."

### C.5 Ralph (`packages/workflow/tool-ralph/src/index.ts`)

- Fresh child per round, no parent context; only a validated structured report crosses rounds. Defaults `:37-39`: `maxRounds 256`, `maxHandoffChars 16384`, `maxResultChars 16384`; call override cannot exceed the deployment ceiling (`:207-217`); `maxTotalAgents` mirrors the cap in the engine (`:452`).
- Script `:153-176`: `for round in 1..maxRounds`, report status `continue|complete|blocked`; `continue` requires nextSteps and empty blocker, `complete` requires evidence and no nextSteps, `blocked` requires a concrete blocker (`:125-143`); oversize report → error, never truncation (`:144-147`); run ends `budget-limited` at the cap (`:176`).
- Policy text `:410`: "Completion and blockers are worker reports, not independent evaluation." README `:93`: "Only round count bounds aggregate effort — token, price, and elapsed-time budgets are deferred."

### C.6 Repeat-tool reminder (`packages/guard/repeat-tool-reminder/src/index.ts`)

- Config `:46-49`: `thresholds [3, 5, 8]`, `argumentsPreviewChars 500`, `include/exclude` wildcards. Load-time validation `:128-141` (empty, <2, non-integer, duplicate → throw).
- Chain key `(tool, deep-key-sorted JSON args)` per live `Agent` (`WeakMap`, `:173`); identical consecutive call increments, different tracked call resets to 1 (`:194-198`); excluded tools are transparent (neither count nor reset, `:175-179`).
- Fires on `tools/post-execute` (`:213-224`) so denied calls count too ("a model hammering a denied call is exactly the loop worth breaking", `:185-187`); prepends a `{kind:'plugin'}`-sourced user message to `additionalContexts`, never replaces the tool result. First threshold = gentle text (`:63-67`), later = detailed with tool/count/args (`:70-79`).
- Reset on any human message at `agent/pre-step` (`:229-232`).
- Note 2026-07-08 `:58-63,68-69,75-76`: rejected patching the tool result ("makes the logged `tool/result` lie"), rejected blocking at the top threshold ("punishes legitimate identical repeats (polling…)"), rejected a loop-level step budget ("blunter, orthogonal control"), rejected fuzzy matching; accepted costs: in-memory only across resume, compaction does not reset chains, "Each trigger costs reminder tokens on the next request; thresholds bound the frequency."

### C.7 Loop caps

- `packages/core/agent-loop/src/constants.ts:6` `DEFAULT_MAX_PARALLEL_TOOL_CALLS = 10`. No per-turn step cap (rejected, above). `agent.ts:285-290` `max-tokens` is sticky for the turn's end reason (so a later completed step cannot mask it) — and the goal driver disarms on it.
- Note 2026-07-16 `:91`: Claude Code's small-model post-turn evaluator was deliberately not copied; `:126` no reflector, "automatic no-progress heuristics, … stuck-pattern detection … are not implemented. Humans can edit, pause, clear, or resume the goal directly."

---

## Lessons for a todo tool with mid-run nudges and stop-time interception

1. **Keep the todo list off the model's stop logic; put the "keep going" decision in a separate, explicit, capped policy.** All three repos separate the checklist (dsh `todo_write`, Pi extension state) from continuation (prime goal/autonomous, dsh goal rounds). dsh note 2026-07-16 `:22`: no universal loop object; two policies with separate contracts.

2. **Continuation is a queued message at a quiescent boundary, never a mid-step interrupt.** Pi `getFollowUpMessages` (`agent-loop.ts:254-263`), prime `getContinuationMessages` (`:429-441`), dsh `agent.followup()` from an `idle` edge (`goal-round-driver:259-277`). Mid-run nudges use the steer seam (Pi `:251`; prime budget_limit as steer `:2205-2210`).

3. **Human/queued work outranks the automatic nudge, and the nudge rolls back if it lost the race.** prime `queuedActionCount > 0 → []` and epoch rollback (`:3315-3326`); dsh `competingQueued` + stale reservation + pre-step double check (`:284-291`, `:349-414`). Pi types: "Explicit follow-up messages always take precedence" (prime `types.ts:240`).

4. **Charge the cap only when the nudge is actually admitted; keep the reservation in memory, the admission in the log.** dsh: only the durable goal-sourced `user/message` increments `roundsStarted` (note `:80`); prime rolls back `continuationsUsed` on cancelled compaction (`:2852-2866`). A byte-exact invariant over the logged nudge (dsh `invariant.ts:45-58`) makes duplicate/forged nudges detectable.

5. **Terminal stop reasons end continuation; never auto-retry around them.** Pi `error|aborted` → agent_end (`:196`); prime `_stopGoalContinuationForTerminalMessage` (`:1879-1889`); dsh `max-tokens` → disarm, `aborted` → pause, `agent/error` → disarm (`:307-331`, `:246-249`), "No abnormal outcome requests an automatic retry."

6. **Every automatic continuation has a hard count cap plus at least one orthogonal bound.** prime autonomous 3 continuations / 12 turns / 80k tokens / 30 min; dsh 256 rounds (+ blocked codes); Ralph 256 rounds + 16 KiB handoff. Pi caps retries at 3 with backoff. prime goal has *no* count cap (token budget optional) — the outlier, bounded only by `goal.complete()`/user.

7. **Don't count cache-read tokens toward the budget** (`autonomous.ts:184-187`) and **don't rerun a verifier on an unchanged tree** (`:283-303`) — both are recorded incidents of loops burning budget without progress.

8. **Nudge text: assert the objective is unchanged, demand evidence before completion, forbid completing because the budget is ending, and label the objective as data.** prime `goals.ts:212-229`; dsh `prompt.ts:18-23`; both JSON/XML-escape the objective. dsh also tells the model to "leave the goal active for the next round" rather than declare done.

9. **Stop-time interception should ask for one grounded closing message, not hard-stop at the tool result.** dsh bug-fix 2026-08-02: hard stop after `update_goal` read as "stopping mid-sentence"; fix = defer a `<goal_complete>`/`<goal_blocked>` context saying "write the closing message… call no more tools", ending via the ordinary no-tool-call stop; structured wording beat "summarize" in A/B, and no instruction produced fabricated detail.

10. **Gate "blocked/give up" behind a mechanical minimum (dsh `blockedAfterConsecutiveRounds` 3, `tool-goal:299-306`) and separate it from the generous continuation cap**; the count is a floor, "not an evaluator of semantic sameness."

11. **Repeat-call reminders: advisory, escalating [3,5,8], post-execute (denied calls count), reset on human input, exclude bookkeeping tools like `todo_write` so they can't launder the chain, cap quoted args at 500 chars.** Never rewrite the tool result to carry the reminder (audit log must stay truthful).

12. **Display lifetime of the checklist is a UI fold, not a log mutation:** latest `todo/write`, kept through `turn/end`, cleared on the next `turn/start` (dsh 2026-07-28). Don't synthesize an empty write.

13. **Whole-list replace with `{content, status}` only; validate uniqueness and the single-`in_progress` rule at execute with an `isError` result so the model self-corrects; make the parallel policy a required deployment choice, and keep it out of the durable invariant so old logs replay.**

14. **Emit a distinct "settled" event after all automatic continuation is exhausted** (Pi `agent_settled`, `extensions/types.ts:745-747`), because `agent_end` fires before retries/compaction/queued continuation.

15. **A `length` stop with tool calls means every call's args may be truncated: fail them all and let the model re-issue** (Pi `agent-loop.ts:374-407`) rather than executing or stopping.

16. **Forced tool *choice* appears nowhere; forced tool *availability* does** (prime force-adds `ipython` so `goal.complete()` is reachable, `:2034-2054`). If a todo tool must be reachable at stop time, ensure it is in the active tool set rather than forcing a call.

17. **Deferred scheduled nudges are skipped to the next tick, not queued** (prime `recordSkipResult` `:672-696`, claim coalescing `:1573-1598`) — the anti-backlog rule for heartbeats.

18. **Hold continuation while children are unsettled** (prime `:3284-3288`); a parent that delegated and stopped is behaving correctly.
