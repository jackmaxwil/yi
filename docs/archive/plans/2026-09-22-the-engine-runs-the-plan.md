# The engine runs the plan: G0-G4

Landed: D223-D229 (0.286.0-0.292.0).

```
status:  planned 2026-09-22. Lands as docs/plans/2026-09-22-the-engine-runs-the-plan.md
         in G0. Stages G0-G4 land as one stacked PR on #467 (claude/yi-os-f1, 82431aa5),
         one changelog row, one D-row ADR and one forge issue (milestone 11) per stage.
tree:    0.285.0, last decision row D222. D223-D227 below are D-next placeholders.
loop:    one subagent at a time; Fable 5.1 implements a stage, Opus 5 reviews and fixes
         in place; the forge gate is the confirmation (this Mac cannot exec fresh
         binaries reliably). After G2: the 12-trial surface confirmation
         (evals/surface.py, $2) and the four directed activation tasks.
```

## Context

105 recorded eval sessions (F0e, 0.283.0 confirmation) show the model failing where
it must remember a procedure the runtime could execute: `start` after `append`,
`submit` before `done`, `done` after the check, the session todo list beside the
plan's todos, `check.py` after every save. `candidate_submitted` was 0 of 105 before
#469; after the 0.283.0 fixes plan-tool refusals still sat near 9 percent because the
fixes named the mistake instead of removing the decision. This plan deletes the
decisions: the engine starts, submits, verifies and accepts delegated work; the todo
tool is the plan's view; a contract's checker runs on every covered write; a
delegation without a contract cannot be declared. Items 5-8 of the recommendation list
(one schema, surface in context, kernel namespace across abort, edit cheaper than sed)
are separate work and are not in this plan.

Governing constraint: `ops.rs` 1158, `state.rs` 1194, `acceptance.rs` 1185,
`subagent.rs` 1129 lines against the 1,200 cap. Every new function lands in a new
small file; the big files get match arms and call sites only.

## G0. A worktree delegation without a contract cannot be declared (D223)

- `crates/types/src/plan/op.rs:101 TodoSpec`: `#[serde(try_from = "TodoSpecRepr")]`,
  same fields, `TryFrom` refuses `isolation == Worktree && contract.is_none()` with the
  #479 text. Covers `init`, `append`, `set`, `decompose`, `supersede` at parse
  (`plan/tool.rs:371 parse_op` and `plan/request.rs` both land in `bad_args`).
  `Todo`'s own `TryFrom<TodoRepr>` (`doc.rs:461`) stays permissive so format-1 documents
  and `import` still load. `table.rs::check_contracted` is the backstop for
  `Op::Retry { delegation }` only; the whole-plan scan in `validate_plan` (`table.rs:253`)
  becomes unreachable and goes with `validate_shape`, and its test moves to the parser.
- Python: `roles.py:60 Writer(accept: Contract, *, ...)` required first keyword; `Role`
  gains `accept`; `plan.py:452 Plan._spec` raises `TypeError("a worktree Writer needs
  accept=")` before sending.
- Tests: move `a_worktree_delegation_with_no_contract_is_refused_where_it_is_declared`
  to a parse test; new `test_a_writer_without_accept_is_refused_before_the_host`.
- Eval: `plan-child-submit` scenario, zero `contract` refusals over 12 trials.
- Also lands the plan doc `docs/plans/2026-09-22-the-engine-runs-the-plan.md`.

## G1. The engine starts ready delegated todos (D224, amends D212)

- `Actor::Engine` at `ops.rs:52`; `actor_word` (`ops.rs:1119`) renders `engine`;
  `table.rs:139 check_actor`: `Engine => Start | Submit | Done | Fail`. `Host` and
  `Child` arms unchanged (D222's early child submit stays legal).
- The store lease (`ops.rs:551`, directory mutex `store.rs:676`) is not reentrant, so
  the scheduler wraps `apply`: rename `ops.rs:513-582` to `apply_once`; new `apply` =
  `apply_once`, then if `Ok` and the op is not `View`, `schedule::dispatch_ready`.
- New `crates/runtime/src/plan/schedule.rs` (~120 lines): read the plan, take
  `admitted(plan, slots)` (`ops.rs:1112`) filtered to delegated Pending todos, skip a
  label with a pending `spawn_intent` (`dispatch.rs:208`, the `NeedsReconciliation`
  road is never re-recorded) and labels in an in-process refused set keyed
  `(plan, label, attempt)` (a contract-freeze refusal at `ops.rs:843` records once),
  then `apply_once(Engine, Start { label })` for each. One pass per op (a start frees no
  slot and clears no edge). Spawned urls join the outer `Outcome.spawned`; a refusal is
  one `notices` line. `tool.rs:646` already prints `spawned agent://...`.
- Backstop for a resumed session that issues no op: one line in `probe.rs:183 tick`
  calling `dispatch_ready_all` on its blocking thread.
- No opt-out field and no lever: `block` is the hold, `unblock` releases into the
  scheduler. D212 amended: shapes schedule inline (`run=`) work and observe delegated
  work; admission is the only dispatcher for delegated todos.
- Deleted: `Delegate::follow_up` (`ops.rs:349`), `SessionDelegate::follow_up`
  (`dispatch.rs:347-375`), the call at `ops.rs:1044-1046`, the `Stub.follow` log in
  `tests/plan_ops.rs`, the "admissible now: ...; you start each one" text, and
  `illegal_hint`'s "(with a delegation, start hands it to a child)" (`ops.rs:42`).
- `plan/tool.rs:718 DESCRIPTION`: "The other ops step single todos, hand one to a child,
  or park it." and "A todo moves pending, running, done in order..." become "Declare
  todos with contracts and delegations; the engine starts, verifies and accepts
  delegated ones. done closes your own todos." Request budget ratchets down.
- Python: `plan.py:591 Run.launch` stops calling `todo.start()` for delegated todos
  (an owner `Start` on a Running todo is `IllegalStep`); `shapes.py` adopts it.
- Tests that flip (rewrite, never weaken): `plan_ops.rs` `backpressure_holds_a_delegated_todo_pending`
  (init spawns the first; owner start is `IllegalStep`), `width_is_the_family_cap_not_the_host`,
  `start_refuses_unmet_after_edges`, `a_pending_spawn_intent_refuses_a_second_start`
  (engine skips, no second refusal record), `a_ninth_delegated_start_is_refused_by_the_engine_with_the_count`,
  `mutation_is_owner_gated_and_unblock_is_open_to_user_and_host` (Engine arm),
  `dispatch.rs` `a_ready_todo_dispatches_a_child_and_reap_promotes_its_product`,
  `the_follow_up_wakes_an_idle_owner_and_names_the_held_count` deleted,
  `campaign-decompose-and-supersede.json` start steps expect `illegal_step`,
  `table.rs:464` gains the Engine row. New: `an_engine_start_is_journaled_as_engine`,
  `a_blocked_todo_is_not_started`.
- Eval: `plan-width-lever` and `plan-fanout`, 12/12 with zero owner `start` calls.

## G2. A child's finish is its submission and its acceptance (D225, extends D222)

- `subagent.rs:174 SubagentHost` gains `finished: Mutex<Option<Arc<FinishFn>>>`,
  `FinishFn = dyn Fn(String, ChildExit, Option<String>) -> bool + Send + Sync`,
  `set_finished` (late-bound like `lease.rs:96 set_lease_clock`; the host is built at
  `wiring.rs:581` before the engine at `:607`). In `conclude` (`subagent.rs:804`) the
  `(Completed, None)` non-service arm and the `Failed`/`Interrupted` arms consult the
  hook first and return when it takes the child; the `(Completed, Some(question))` arm
  is untouched (D165). `publish` and `attribute` stay above.
- New `crates/runtime/src/plan/finish.rs` (~200 lines), installed by `wire_plan_engine`
  at depth 0: `locate(agent)` scans active roots for a todo `Running { by == agent }`
  with a delegation; none means an `rlm.run` child and conclude's own notice runs;
  found means `spawn_blocking(engine.child_finished(...))` (it forks `/bin/sh`).
  - Completed: product = `last_assistant_text` via `host.transcript(agent)`
    (`dispatch.rs:323`) stored through the same artifact store `request.rs:198-202`
    uses (`plan://<id>/artifacts/<digest>`), uniform for worktree and inline (worktree
    `submit` needs an `output` url, `op.rs:233`). `phase_of` (`acceptance.rs:122`):
    `Unsubmitted` means `apply(Engine, Submit)` (worktree: `submit_candidate`,
    `candidate_of` under the lease, verify, integrate; inline: `apply_submit`);
    `Submitted..IntegrationStale` (child submitted early) skips to done;
    `Accepted`/`Disposed` does nothing. Then `apply(Engine, Done { output })`
    (`accept` at `acceptance.rs:733`; `try_publish`'s quiescence wait is satisfied by
    the exit; inline `done.rs:92`). A `Fail` verdict means
    `apply(Engine, Fail { cause: "contract refused: <items>", disposition: Retained })`
    (mirrors `plan.py:609 Run._complete`). `Abstain`/`Escalate`/`MergeFailed` leave the
    todo Running by the exited child; the owner gets the verdict and keeps
    `fail`/`retry`/`accepted_by_user`. That is the one decision left to the owner, on
    the abnormal path only.
  - Failed/Interrupted: `apply(Engine, Fail { cause: error, disposition: Retained })`.
  - Notice: one message through `SessionDelegate::say` (`dispatch.rs:160`,
    `plan_relevance`, `Steer`, proven to wake an idle owner by `dispatch.rs:830`):
    `plan: accepted "alpha" (agent://...)`, `plan: refused "alpha": <item>: <detail>`,
    `plan: failed "alpha": <cause>`. The `done` reaps the child (`ops.rs:887`), so the
    promoted transcript and the `history://` pin still reach the owner.
- `done.rs:686 admit`: `by != txn.actor && txn.actor != ENGINE_AGENT`.
- `INTEGRATION` (`acceptance.rs:599`) serializes against an owner's concurrent `done`.
  Crash before the engine's submit: `Liveness::alive == Some(false)` (`dispatch.rs:177`)
  is the existing repair road; no re-drive is built.
- `brief()` `dispatch.rs:62-75`: both submit lines deleted; test
  `a_worktree_brief_names_the_submit_verb` deleted, `a_brief_never_names_submit` added.
- `doctrine.md:52-69` cut to the todo-tool-only sentence; `:258-259` deleted; `:341-342`
  becomes "the engine starts each delegated todo, submits its child's finish and accepts
  or refuses it; you read the result." `help(yi)` (`yi/__init__.py:4`): "Todo: done (your
  own), fail, retry, block, result; delegated ones the engine steps."
- `plan/mod.rs:329-358 stale_reminder`, `StaleTracker`, `stale_turns` deleted (a ready
  delegated todo is never unclaimed now).
- Python: `Run.settle`/`_complete` read state for delegated todos and send no `done`;
  `Todo.submit`/`Todo.done` stay for inline todos and early submit.
- Tests: `plan_e2e.rs` `a_child_may_submit_only_for_its_own_attempt` and
  `a_child_stores_the_product_of_the_attempt_it_submits` kept (early submit then
  auto-accept); `dispatch.rs a_failed_childs_last_product_survives_the_reap` (fail is the
  engine's); `test_yi_plan test_a_refused_done_fails_the_attempt` moves to the engine
  path. New: `a_finished_worktree_child_is_accepted_without_an_owner_op`,
  `a_red_contract_fails_the_todo_retained`, `an_asking_child_is_not_submitted`,
  `an_rlm_run_child_is_not_a_plan_finish`.
- Eval: `plan-child-submit`, `plan-fanout` 12/12 with zero owner `submit`/`done` on
  delegated todos; `toolbox-port` and `fleet-forensics` directed: acceptance by the
  engine, the parent never runs `check.py`.

## G3. One todo register (D226, amends D137)

- Write-through mirror over the `OpSink` seam (`ops.rs:364`, `emit` at `:621`), which
  already fires once per committed op; a projection would route every `TodoStore::list()`
  reader (environment, ACP, TUI, console, `text.rs`) through the engine. `TodoItem.state`
  is already `TodoStateName` (`types/src/todo.rs:119`) with a flattened `extra` map, so
  the plan projects with no wire change.
- New `crates/runtime/src/todo/mirror.rs` (~90 lines): `Mirror { inner, todos, store }`
  implements `OpSink`: forward, then `store.read(plan)`, `projected(&plan, &current)`,
  `todos.replace(list, ENGINE_ACTOR)`. One phase named by the plan id; one item per todo
  (`label`, `state`, `note` = blocked note or fail cause, `extra.plan`, `extra.by`);
  ids kept by label so `t{n}` stays stable; sub-plan records project the root.
- `todo/mod.rs`: `replace(list, actor)` beside `apply_as` (~15 lines); `apply_as`
  (`:220`) refuses every `is_state_change()` op when `list.extra.plan` is set and the
  actor is not the engine, `TodoError::Mirrored { plan }`: "the list is plan {plan}; the
  plan tool changes it (append, block, drop; the engine steps delegated todos)". One
  guard covers tool, seed and ACP.
- Wiring: the `TodoStore` is created in `wire_plan_request` (`wiring.rs:314`) and
  passed to `with_op_sink(Mirror)`; `environment.rs:192-198` unchanged (the header now
  shows the plan). `loop_coupling.rs:140 reinjection_text` drops `frontier_text`, keeps
  `summary_line`.
- Tests: `a_plan_op_replaces_the_list_and_keeps_ids`,
  `a_mirrored_item_refuses_start_with_the_plan_road`, environment header shows plan
  counts. Eval: `plan-fanout` gains `noRefusalsFrom: ["todo"]`.

## G4. A contract's checker runs after every covered write (D227, extends F0c)

- `contract.rs:274 Contract` gains `#[serde(default, skip_serializing_if =
  "Vec::is_empty")] pub covers: Vec<String>` (globs relative to the checkout);
  existing digests unchanged.
- `tools.rs:13 ToolAdapter` gains `check: Option<Arc<dyn Fn(&Path) -> Option<String>>>`
  read from a new `AgentSession` slot like `rules_engine()` (`session.rs:505`). At
  `tools.rs:295` capture `args.path` resolved against `context.cwd` before `args` moves;
  after the affordance lines (`:319`), for `ToolKind::Write` and no error, run the check
  in its own `spawn_blocking` and `affordance::append` the line.
- New `crates/runtime/src/plan/covers.rs` (~150 lines): `PlanEngine::preview(actor,
  path, cwd)`: Running todos whose contract `covers` the path (an `Actor::Child(me)` sees
  only its own todo); for each `Decider::Cmd` run `verify.rs:423 run_cmd` with the frozen
  manifest against the live tree at `cwd`, deadline from the manifest capped at
  `DEFAULT_CHECK_TIMEOUT_MS`; debounce by `sha256(path, bytes)` per `(label, item)`.
  Output `contract "alpha" check: pass` or `fail: exit 1: <tail>`. No journal record,
  no `VerificationToken`, no `protected_digests`: a preview, never an acceptance.
  Glob matching: `globset` if already in `Cargo.lock`, else prefix plus `*.ext` suffix.
- Installed at every depth (the child engine reads the same `plans_dir`,
  `wiring.rs:534-538`).
- Tests: `a_covered_write_runs_the_cmd_item_and_debounces`,
  `a_contract_without_covers_keeps_its_digest`; `request_budget` unchanged.
- Eval: `edit-file` with a `covers` contract, verdict line in at least 11 of 12 write
  results.

## Not built

No opt-out field (block is the hold); one scheduling pass per op; the preview runs
`Cmd` items only; the refused set is per process; no re-drive of a finished but
unsubmitted child after a crash (repair stands); items 5-8 of the recommendation list.

## Verification

Per stage: `just check` locally where the Mac can exec the binary, the forge gate on
the PR otherwise; every flipped test rewritten to pin the new rule; request budget
and `tool_surface.json` move only through their own `Ratchet` commits. After G2 and
after G4: `evals/surface.py` 12 trials against the named scenarios, and the four
directed activation tasks under `evals/fixtures/tasks/*` with the same driver as F0e;
the target is zero owner `start`/`submit`/`done` on delegated todos and zero plan-tool
refusals in the confirmation.

Measured (docs/eval-ledger.md rows 0057-0059, recorded 2026-09-23): the confirmations
ran after G2 (059ba86b, $2.08), after G4 (d4f8e62d, $2.49) and after G5 (e15ab4d1,
$2.00). G2b and G5 are stages this plan did not name; each came from the previous
confirmation's failure classes. The targets above were not all met. Owner steps on
delegated todos fell 93, 13, 4 and never reached zero; plan-tool isError was 52.2,
46.5 and 23.9 percent; G0's declaration refusals for a missing contract were 58, 7
and 2; the todo tool still refused in `plan-fanout` in the last run; G4's `edit-file`
check was never run as written (a `plan-covered-write` scenario carried the verdict
line on 3 of 3, then 4 of 4 covered writes); and the parent ran `check.py` or
`pytest` itself in 3 of 4, 8 of 8 and 8 of 8 directed trials, which G2's eval line
says it never does. Directed
rewards were 1/4, 1/4 (3/4 with `--here`) and 4/8.
