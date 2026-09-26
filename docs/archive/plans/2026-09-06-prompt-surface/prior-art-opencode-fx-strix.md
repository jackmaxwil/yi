# Prior art: todo tracking + "keep working until done" in opencode, fx, strix

Read-only survey, 2026-09-06. Paths are relative to
`~/Development/yi/ref/agents/<repo>/`. All three clones are
shallow (one commit); none carries a CHANGELOG with todo entries except fx,
whose CHANGELOG has no todo tool at all. No tests, node_modules, vendor or
benchmarks dirs were opened.

Headline: only opencode and strix have a todo tool; neither reads the todo
list back at runtime. No repo nudges on "open todos" or "long since last
update". Every runtime nudge found is keyed on something the loop can observe
without the model's cooperation: turn ended with no tool call (strix),
identical tool call repeated (opencode), all-malformed / all-denied batch
(fx), step or budget ceiling (all three).

---

## A. opencode (TypeScript, commit 15537a4, 2026-08-27)

### A.1 Tool: `todowrite` (there is no `todoread` any more)

- Schema `packages/schema/src/session-todo.ts:7-15`: `{content, status,
  priority}`; `status` and `priority` are plain `Schema.String` — the valid
  values (`pending, in_progress, completed, cancelled`; `high, medium, low`)
  live only in the field description. No id, no parent, no nesting. Event
  `todo.updated` `{sessionID, todos}` at :18-25.
- v2 tool `packages/core/src/tool/todowrite.ts:12-56`: input is the whole
  list (`todos: Array<Info>`), execution = permission assert on action
  `todowrite` + `todos.update(sessionID, todos)`; model sees the list back as
  pretty JSON (:23). Replace-whole-list semantics, no merge.
- v1 tool `packages/opencode/src/tool/todo.ts:14-46`: same, plus a title of
  `"<N> todos"` counting non-completed items (:37).
- `todoread` survives only as a doc ghost: `packages/web/src/content/docs/
  agents.mdx:442` still maps permission key `todowrite` to `todowrite,
  todoread`; no such tool exists in `packages/core/src/tool/` or
  `packages/opencode/src/tool/`.
- Subagents: `todowrite` denied by default (`packages/core/src/plugin/
  agent.ts:156`, `packages/opencode/src/agent/agent.ts:188`; docs
  `tools.mdx:231`). Only the top-level agent keeps a list.

### A.2 Prompt text (the actual rules)

`packages/opencode/src/tool/todowrite.txt:1-44`:
- Use when: 3+ distinct steps; non-trivial work; user gives multiple tasks or
  asks for a list; new instructions arrive; on start mark `in_progress`
  ("only one at a time"); on finish mark `completed` and add follow-ups.
- Skip when: single task or <3 trivial steps; informational; no value.
- Rules (:24-30): update in real time, don't batch completions; `completed`
  only after verification, "never based on intent"; exactly one
  `in_progress` while work remains; if blocked keep `in_progress` and add a
  blocker todo; preserve user commands verbatim; "When in doubt, use it."

System-prompt reinforcement, per model family
(`packages/opencode/src/session/prompt/`):
- `anthropic.txt:23-27` "Use these tools VERY frequently … If you do not use
  this tool when planning, you may forget to do important tasks - and that is
  unacceptable … Do not batch up multiple tasks"; `:96` "IMPORTANT: Always
  use the TodoWrite tool".
- `meta.txt:36-40` adds the one line that ties todos to continuation: "Work
  through the whole todo list to completion in one turn, marking items done
  as you go."
- `beast.txt:1,5,9,22,28,79` (GPT-4.1 style): "keep going until the user's
  query is completely resolved, before ending your turn"; "Only terminate
  your turn when … all items have been checked off"; on "resume/continue/
  try again" find the next incomplete todo and "do not hand back control to
  the user until the entire todo list is complete"; "Make sure that you
  ACTUALLY continue on to the next step after checking off a step instead of
  ending your turn and asking the user what they want to do next." (beast
  uses a markdown checklist, not the tool.)
- `copilot-gpt-5.txt:6-10,32-33,55`: same "keep going" preamble plus
  "CRITICAL - Before ending your turn: Review and update the todo list,
  marking completed, skipped (with explanations), or blocked items."
- `gpt.txt:19` "Persist until the task is fully handled end-to-end within the
  current turn"; `gemini.txt:155` "keep going until the user's query is
  completely resolved"; `plan-mode.txt:65` "your turn should only end with
  either asking the user a question or calling plan_exit."

### A.3 Persistence

- `packages/core/src/session/todo.ts:32-57` (v1 twin at
  `packages/opencode/src/session/todo.ts:29-51`): one SQLite transaction —
  `DELETE WHERE session_id` then bulk `INSERT` with a `position` column;
  publishes `todo.updated`. `get` orders by `position` (:59-72).
- Table `packages/core/src/session/sql.ts:100-112`: `todo(session_id FK
  cascade, content, status, priority, position, timestamps)`. Per-session,
  flat, dies with the session. Not part of the message stream; survives
  compaction untouched (compaction never reads it).

### A.4 Rendering

- TUI item `packages/tui/src/component/todo-item.tsx:19`: `[✓]` completed,
  `[•]` in_progress (warning colour), `[ ]` otherwise; cancelled is not
  distinguished.
- TUI sidebar plugin `packages/tui/src/feature-plugins/sidebar/todo.tsx:11-28`:
  live widget fed by `api.state.session.todo(session_id)`; shown only while
  `list.length > 0 && some(status !== "completed")` (:12) — the list
  disappears the moment everything is done; collapsible header only when
  more than 2 items (:17-19), default open.
- TUI transcript renderer `packages/tui/src/routes/session/index.tsx:2525-2549`:
  each `todowrite` call renders as a `# Todos` block with the full list; while
  pending shows "Updating todos...".
- Web app dock `packages/app/src/pages/session/composer/session-todo-dock.tsx`:
  collapsible tray in the composer; header shows `done/total` with animated
  numbers (:58-60) and a one-line preview of the "active" item = first
  `in_progress` ?? first `pending` ?? last `completed` (:67-73); list uses a
  read-only checkbox, indeterminate for `in_progress` with a pulsing dot
  (:236-246), strike-through for `completed` or `cancelled` (:248-262),
  scrolling area capped at `max-h-42`.

### A.5 Continuation and stop-time: what the loop actually does

Session loop `packages/opencode/src/session/prompt.ts:1088-1340`:
- Exit condition (:1101-1130): last assistant message has a `finish` reason
  other than `tool-calls`/`unknown` AND contains no tool parts. Comment at
  :1103-1105: "Some providers return 'stop' even when the assistant message
  contains tool calls. Keep the loop running so tool results can be sent
  back." Nothing here reads the todo table. There is no open-todo check at
  stop time and no "you have not updated todos in N steps" nudge anywhere
  (grep for `todo` across `session/*.ts` and `runner/*.ts` finds only the
  store).
- `SessionReminders.apply` (`packages/opencode/src/session/reminders.ts:15-90`)
  is the only synthetic-part injector: plan-mode / build-switch prompts
  appended to the last user message. Not todo-related.
- Subtask return (:430-447): after a `task` subagent finishes, a synthetic
  user message "Summarize the task tool output above and continue with your
  task." is appended — a continuation nudge keyed on an event, not on state.
- Step cap: `agent.steps ?? Infinity` (:1178); on the last step
  `MAX_STEPS_PROMPT` (`packages/core/src/session/runner/max-steps.ts:1-16`) is
  appended as an *assistant* message (:1281): "MAXIMUM STEPS REACHED … Tools
  are disabled until next user input … List of any remaining tasks that were
  not completed". Docs `agents.mdx:291-313`: default is unbounded ("the agent
  will continue to iterate until the model chooses to stop or the user
  interrupts").
- Failure-mode evidence in code comment (:1297-1300): content-filter
  finishes "may have produced no visible output at all — previously the
  session went idle silently"; now surfaced as an error and the loop breaks.

### A.6 Loop detector: `doom_loop`

`packages/opencode/src/session/processor.ts:29,353-378`: on every tool-call
start, take the last `DOOM_LOOP_THRESHOLD = 3` parts of the current assistant
message; if all three are tool parts with the same tool name, not pending,
and `JSON.stringify(input)` byte-equal to the new call, raise a permission
ask `doom_loop` (patterns `[tool]`, `always: [tool]`). Default action `ask`
(`packages/opencode/src/agent/agent.ts:121`; docs `permissions.mdx:164,173`
"triggered when the same tool call repeats 3 times with identical input").
UI copy `cli/cmd/run/permission.shared.ts:111-117`: "Continue after repeated
failures — This keeps the session running despite repeated failures." Bound:
a human decision, and "always" lifts it for that tool for the session.

---

## B. fx (Zig, CHANGELOG 0.0.5)

### B.1 No todo / plan tool

Builtin tool list `src/builtins/tools.zig:510-1058`: list_files, glob_files,
grep_files, read_file, write_file, edit_file, delete_file, rename_file,
copy_file, create_folder, file_info, memory, semantic_search, open_file,
web_fetch, web_search, terminal, skill, plus `ask_user_question` (:1239-1249,
1–4 questions, optional `permission_request_id` echoing an auto-denied tool
result so the model can appeal a denial). `src/core/tasks/task_helpers.zig` is
background-process bookkeeping (task = OS process), not a checklist. Nothing
in README/AGENTS mentions todo, plan mode or task lists. Grep of `src/` for
`todo` yields only TODO-string test fixtures.

### B.2 Continuation: none automatic; explicit hard stops with notices

- Step limit `src/core/config/agent_steps.zig:3,23-25`: default 0 =
  unbounded (`allowsStep(limit, n) = limit == 0 or n < limit`); the napi/wasm
  hosts pass 64 (`src/napi_core_main.zig:449`, `src/wasm_core_main.zig:37`).
  On exhaustion `finishFailedTurnWithNotice` (`src/core/agent/runtime/
  orchestrator.zig:7217-7229, 7280-7310`) pushes an *operational* text line
  (`config.zig:20`): "Agent step limit reached; continue with a follow-up
  prompt if needed." and materialises the retained assistant candidate as a
  failed terminal. Continuation is the user's job.
- Malformed-argument loop `src/core/agent/runtime/tool_admission.zig:34,349-376`:
  `MalformedArgumentsRetryState` counts *batches* where every call had
  `argument_integrity == .malformed_json`; any batch with one valid call
  resets the counter to 0; the third consecutive all-malformed batch ends the
  turn with `repeated_malformed_arguments_notice` (`orchestrator.zig:68-69`,
  fired at :7015-7035): "Repeated malformed tool arguments stopped the agent
  loop. The invalid calls were not executed. Continue with a follow-up prompt
  if needed." CHANGELOG.md:44 "Malformed tool loops: End a turn after three
  consecutive malformed-only tool batches and reset recovery after a valid
  batch."
- Auto-denial loop `orchestrator.zig:2226-2268`: `max_automatic_denial_
  response_groups = 4`; `automatic_recovery_disposition` walks the
  within-turn suffix, groups each assistant tool-call message with its
  results, and counts consecutive groups whose only denial reason is
  `auto_denied`; a group with any `success` or any non-auto denial resets to
  0. At 4 the turn finishes "with normal blocker" (:2818-2830) instead of
  opening a permission prompt. CHANGELOG.md:30 "finish repeated no-progress
  denials as normal assistant output instead of opening a permission
  prompt"; :118 "return a tools-disabled response after repeated blocks
  instead of stalling for approval". Per-turn denial memory hard cap
  `max_turn_permission_denials = 64` (`tool_admission.zig:33,74`).
- Terminal correction loop `tool_admission.zig:300-347`: sha256 of each
  field-correction message; if a digest recurs from the previous batch,
  `stop_after_batch` — one correction, never the same repair twice.
  CHANGELOG.md:122.
- Historical-state guard `src/core/agent/runtime/stop_policy.zig:15-37`:
  blocks re-starting a background command the runtime context says is dead
  unless the prompt explicitly asks (phrase heuristics :94-115). Tool result
  text tells the model to "Answer from that state".

fx has no mid-run nudges and no idle timers; every guard is a counter on
observable batch outcomes with a fixed small cap, a reset on any real
progress, and a one-line operational notice that hands control back.

---

## C. strix (Python, openai-agents SDK)

### C.1 Todo tools (six, per-agent, disk-mirrored)

`strix/tools/todo/tools.py`:
- Statuses `pending | in_progress | done` (:21); priorities `low | normal |
  high | critical` (:20); unknown priority coerces to `normal` rather than
  failing (:119-123); unknown status is an error (:254-259).
- Per-agent private lists keyed by `agent_id` (:103-109); subagents never see
  the parent's list. Ids `uuid4()[:6]` (:328). Flat; no nesting, no ordering
  field — sort is status (done → in_progress → pending) then priority then
  `created_at` (:23-32).
- `create_todo` dedupes by lowercased title against existing and within the
  call; skipped titles are returned under `skipped` (:317-325).
- Every mutation returns the full sorted list + `total_count` (:348-359,
  :487-496, :524-534) so the model never needs a separate read.
- Tools: `create_todo`, `list_todos(status?, priority?)`, `update_todo`
  (bulk), `mark_todo_done`, `mark_todo_pending` ("e.g., to retry a failed
  task" :552), `delete_todo` (hard delete :565). All take JSON-string args and
  accept sloppy input (bare string, comma list, bullet lines :134-228).
- Persistence: `{state_dir}/todos.json`, atomic tmp+`replace` (:78-100),
  hydrated on resume (:41-75; called from `strix/core/runner.py:267-269`).
  Unreadable file → log + start empty (:48-55).
- Prompt guidance `strix/agents/prompts/system_prompt.jinja:249`: "your own
  working checklist for a multi-step task. Create todos when your task has
  several distinct steps so nothing is dropped across a long run; mark them
  done as you finish. This is private working memory — use `notes` for
  anything another agent needs." Root role (:9, :348, :354): "tracking
  todos/notes/coverage".
- Tool docstring (:275-284): use for multi-step assessments with parallel
  workstreams; skip for "Simple linear workflows where progress is obvious"
  and "Single quick task — just do it."

### C.2 Rendering

- Go TUI `strix/interface/tui/internal/render/todo.go:13-80`: per-call card
  titled by action (Todo / Todos / Todo Updated / Todo Completed / Todo
  Reopened / Todo Removed), markers `[ ]` pending, `[~]` in_progress
  (italic), `[•]` done (dim + strikethrough); prints the full list returned
  by the tool. No persistent sidebar; the list is visible wherever the last
  todo call sits in the transcript.
- Web viewer `strix/interface/viewer/frontend/src/components/live/
  tool-renderers/TodoRenderer.tsx:12-19`: `list_todos` is labelled "Plan",
  mutations "Task added/updated/completed/reopened/removed"; the affected
  `todo_id` row is highlighted (:32,94); `in_progress` icon pulses.

### C.3 Continuation: lifecycle tools + bounded nudge budget (the meat)

Design (`system_prompt.jinja:36-56`): the turn never ends on text. Ending
requires a lifecycle tool — `respond_to_user` (yield to human),
`wait_for_agents` (park on children), `agent_finish`/`finish_scan`
(terminate). "A turn that ends with plain text and no tool call does NOT
stop you: the system nudges you to continue and will re-run you." Anti-spam
line at :43: "If you do end a turn on plain text and the nudge arrives, your
words already reached the user. Do not restate them: call respond_to_user
with NO message to simply wait." Anti-think-loop line at :47: "Never loop
through think or other tools just to prepare, polish, confirm, or announce
an answer."

Mechanism `strix/core/execution.py`:
- `_finish_tool_use_behavior` (`strix/agents/factory.py:538-560`): the SDK
  run ends only when a lifecycle tool returns `success` + its completion key
  (:507-521) or, interactive only, a parking tool returns
  `wait_outcome == "waiting"` (:524-535). Plain text is never final output.
- `_run_until_lifecycle` (:455-530): after each cycle, if the agent's
  coordinator status is still `running` (no lifecycle tool fired),
  `record_recovery` increments a per-agent counter and a *user-role* message
  is appended (:831-866): "Your previous message ended a turn without a tool
  call. Plain text never ends execution … Continue immediately and call
  exactly one tool. If you have something to tell the user and nothing to do
  until they reply, call respond_to_user — with no message if you have
  already said it … This is recovery attempt {n}/{limit}." Autonomous variant
  says the text "is ignored".
- Cap: `_INTERACTIVE_TOOL_RECOVERY_LIMIT = 3` (:448); autonomous runs use
  `max(1, max_turns)` (default `DEFAULT_MAX_TURNS = 500`,
  `strix/config/settings.py:13`).
- Exhaustion `_exhausted_recovery` (:533-570): interactive → park as
  `waiting/stalled` and send parent `_STALL_NOTICE` (:894-898 "kept ending
  turns without a tool call and is parked until it receives a message …
  either message it with a concrete next step to unblock it, or stop waiting
  on it"); autonomous → status `crashed`, raise `MaxTurnsExceeded`
  ("Agent exhausted recovery attempts without calling finish_scan or
  agent_finish"). Docstring :539-542: "Interactive runs park instead of
  dying: a human is attached … Autonomous runs have nobody to resume them, so
  they fail loudly."
- Budget reset only on real progress: `reset_recovery` on any lifecycle tool
  (:512) or real inbound message (:259-262, comment: "Real input is real
  progress, so the nudge budget starts over. A bare auto-resume is not: it
  must not hand a wedged agent a fresh budget"). Counters are persisted in
  the coordinator snapshot (`strix/core/agents.py:210-226`, docstring:
  "Persisted so a resumed agent cannot earn a fresh nudge budget on every
  auto-resume and loop forever").
- Idle auto-resume (:577-606): a parked agent is re-woken on a 300 s timer
  only when `wait_kind == "agents"` (never when waiting on a human), with
  `_MAX_IDLE_AUTO_RESUMES = 3`; past that it is parked as `stalled` and the
  parent notified (:264-274). Comment :575-576: "An agent that parks again
  after every auto-resume makes no progress, so stop spending a model turn
  per timeout."
- Backstop logging `strix/core/runner.py:600-608`: a scan whose final output
  is not a `finish_scan` result logs "ended without calling finish_scan …
  emitted a text-only turn instead of a lifecycle tool call, so no executive
  report was written."

### C.4 Wind-down nudges keyed on budget, not on todos

`strix/core/hooks.py`: `on_llm_start` (:143-152) appends a user item every
LLM call once a band is crossed — turn bands 70/85/95 % of `max_turns`
(:26,159-178: "About N turn(s) remain before this agent is force-stopped and
any in-progress work is discarded"), cost bands 70/85/95 % root and
75/80/85 % sub-agents with a 90 % sub-agent reserve (:27-29,180-224).
Escalating directives (:68-104) NOTICE → URGENT → CRITICAL, e.g. root at
stage 2: "STOP all other work on the whole scan and finish immediately:
secure your findings and call finish_scan now — anything left unfinished when
the limit is hit is discarded." There is no de-duplication: past the band
the warning is re-injected on every LLM start (only the stage escalates).

### C.5 Stop-time honesty without reading todos

`agent_finish` (`strix/tools/agents_graph/tools.py:531-566`) takes
`open_items` and the docstring insists "anything you could neither confirm
nor rule out belongs in `open_items` … Pass an empty list only when nothing
is genuinely left open." Neither `agent_finish` nor `finish_scan`
(`strix/tools/finish/tool.py:18-46`, which only validates four non-empty
report sections) consults the todo store. Open todos at finish are silently
abandoned.

---

## Lessons for a todo tool with mid-run nudges and stop-time interception

1. Nobody nudges on todo state. All three key runtime nudges on facts the
   loop observes without trusting the model: no-tool-call turn
   (strix `execution.py:455-530`), identical call ×3 (opencode
   `processor.ts:29,356`), all-malformed ×3 / all-auto-denied ×4 (fx
   `tool_admission.zig:34`, `orchestrator.zig:2226`). A todo-state nudge
   would be new ground; the closest analogue is strix's budget wind-down
   (`hooks.py:143-224`), which escalates a three-stage directive and reads
   only a counter.
2. Give every nudge a small fixed cap and a reset on real progress. strix:
   3 recoveries, reset on any lifecycle tool or real inbound message, never
   on an auto-resume (`execution.py:259-262,448,512`); fx resets its
   malformed/denial counters on any successful call in the batch
   (`tool_admission.zig:368-370`, `orchestrator.zig:2257-2261`). Persist the
   counter with the session so a resume cannot mint a fresh budget
   (`agents.py:210-226`).
3. Say in the nudge what number it is and what ends it: "This is recovery
   attempt {n}/{limit}" and the named exits (`execution.py:840-850`). fx
   notices end with "continue with a follow-up prompt if needed"
   (`config.zig:20`). Bare "keep going" text with no exit is what the beast/
   copilot prompts rely on, and those have no runtime enforcement at all.
4. Preempt the restate-spam failure. strix's prompt line 43 plus
   `respond_to_user(message="")` (`respond/tool.py:18-50`) exist because a
   nudged model re-sends its last paragraph; the fix was to make "yield
   without saying anything" a legal call. Any stop-time interception that
   re-runs the model needs the same escape hatch.
5. When the cap is hit, hand back, don't loop: fx materialises a failed
   terminal with an operational line; strix parks (interactive) or crashes
   (autonomous) and tells the parent (`_STALL_NOTICE`). None re-prompts past
   the cap.
6. Persist the list outside the message stream (opencode SQLite table with
   `position`, `session/sql.ts:100-112`; strix `todos.json` atomic replace,
   `tools.py:78-100`) so compaction and re-rendering cannot lose it; replace-
   whole-list (opencode) is simplest, per-item ids + dedupe by title (strix)
   is what you need for partial updates.
7. Return the full list from every write (strix `tools.py:354,491`; opencode
   `todowrite.ts:23`) so a separate read tool is unnecessary — opencode
   deleted `todoread` and only a doc row (`agents.mdx:442`) remembers it.
8. The prompt rules that matter, verbatim across repos: exactly one
   `in_progress`; mark `completed` only after verification, "never based on
   intent"; blocked → keep `in_progress` and add a blocker item; "Before
   ending your turn: review and update the todo list, marking completed,
   skipped (with explanations), or blocked" (`todowrite.txt:24-30`,
   `copilot-gpt-5.txt:32-33`). A stop-time interceptor can check exactly
   these invariants mechanically: any `in_progress` or `pending` item when
   the model stops without a tool call is the trigger; more than one
   `in_progress` is a tool-input error.
9. Live widget rule from opencode: show the sidebar only while some item is
   not completed (`sidebar/todo.tsx:12`), collapse header only past 2 items,
   and render the "active" line as `in_progress ?? first pending ?? last
   completed` (`session-todo-dock.tsx:67-73`). strix has no live widget —
   the list lives in the transcript card of the last todo call.
10. Deny the tool to subagents by default (opencode `plugin/agent.ts:156`) or
    scope lists per agent (strix `tools.py:103-109`); a shared list across a
    subagent tree was not attempted by anyone.
11. Reference for a stop-time report shape: opencode's `MAX_STEPS_PROMPT`
    (`max-steps.ts:1-16`) demands "List of any remaining tasks that were not
    completed" — a step-cap interception that already asks for the todo
    delta in prose. Emitting the actual open items from the store instead of
    asking the model to recall them is the obvious upgrade.
