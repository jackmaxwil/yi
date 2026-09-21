# Yi, an operating system for agent work: the kernel, the user space, the store, the ledger

```
status:  planned 2026-09-12; revised 2026-09-13 after an external review (§0).
         Lands as docs/plans/2026-09-12-yi-operating-system.md (stage F0-S0).
         Stages F0-F4 land as stacked PRs, in order; F0e records a correctness,
         activation and value decision in docs/eval-ledger.md before F1 starts.
         Owner's answers of 2026-09-13: the journal stays in .yi/plans/<slug>/;
         the original format-1 bytes are kept as an artifact blob; F1b ships
         fork_join and scatter; the value gate's tolerance is 10 percent.
tree:    0.244.0 (docs/ARCHITECTURE.md:4), last decision row D190 (:146);
         D169-D171 are in the log (:164-166) and under docs/solutions/adr/.
         Every D below is a D-next placeholder; claim against the header at
         land time (.ruler/090-workflow.md:3-7). #378 still claims D191;
         main reached 0.264.0 while F0a-F0d were in flight, so on the
         rebase they claim D192-D195 and 0.265.0-0.268.0.
         Worktree HEAD ee336f6f, 394 commits past 2d1d7aae; anchors re-
         verified 2026-09-13 (§2 at 2d1d7aae, §2.1 at ee336f6f, the tree
         wins where they differ).
         Cargo workspace.package version is 0.2.0 (Cargo.toml:22); the
         header version is the one rows and D-claims key on.
sibling: docs/plans/2026-09-13-kernel-target.md (PR #421, Revision 5)
         replaces the Jupyter kernel with a Yi-owned CPython subprocess,
         per-cell history and object handoff. F0 here is kernel-side Rust
         and touches the kernel only through the host registry, so it is
         independent of that plan; F1 (the yi library) is re-anchored
         against whichever of the two lands first.
issues:  one per stage under one milestone ("An operating system for agent
         work"; the owner names it), each opened before the stage's changelog
         row (D106; .ruler/095-tracking.md:3-16). Titles in §11.
evidence: the tree at 2d1d7aae, every claim path:line, re-verified 2026-09-12
         (§2 records where the tree disagreed with the anchors handed in);
         docs/eval-ledger.md rows 0018-0025; the five papers and
         docs/plans/2026-09-01-hive.md read 2026-09-11/12; six Exa agent
         runs of 2026-09-11 (scratchpad exa/run-*.json, 109 sources).
lineage: docs/plans/2026-08-31-four-primitives-and-a-plan-engine.md (§3.1
         store, §5 rules, §11 killed, §12 ledger yield); docs/plans/2026-09-01-
         hive.md (§7.4 capsules, §7.5 placement, §8 laws L1/L4/L6, §11 test
         the control); docs/plans/2026-09-08-pass-levers.md (S8-S10, laws);
         docs/plans/2026-09-06-tbv4-evals.md (the instrument, D140);
         docs/plans/2026-09-06-prompt-surface.md (laws 1-6, D137-D139);
         docs/YI_DESIGN.md §8.10, §8.17, §8.17.1; docs/solutions/adr/d77.md.
```

## 0. Revision 2 disposition (2026-09-13)

An external review ("Revision 2") of this plan was received on 2026-09-13
with thirteen source findings (R1-R13) and a rewritten stage order. Every
finding was re-verified against the tree at 2d1d7aae; the table records
what this plan accepts, modifies or rejects. Sections below that the
disposition amends are marked in place at their next rewrite; until then
this table wins where the two disagree.

| id | reviewer's finding | verified at | disposition |
|---|---|---|---|
| R1 | `do_set` constructs `TodoState::Done { output: None }` directly, so a verifier only in `do_done` leaves a bypass | `ops.rs:1007-1041` (`TodoStateName::Done => TodoState::Done { output: None }`) | **accepted.** §6.5: every completion path (done, set, import, repair, supersede, CLI) goes through one validator; `set` may declare and rearrange, never author Done |
| R2 | `framed` writes then emits; `OpSink::record` returns `()`; a second sink does not make snapshot and log atomic | `ops.rs:535-541`, `ops.rs:255-257`, `ledger.rs:19` | **accepted.** The op log becomes the commit point (append + flush, one record per transaction) and `plan.json` a checkpoint written after it; the session sink stays telemetry |
| R3 | `do_start` spawns inside `mutate`, before `write_all` persists; a crash between them orphans a child | `ops.rs:535-539` (spawned children are reaped only when the write fails) | **accepted, scoped.** A `spawn_intent{attempt}` record precedes `delegate.spawn`, a `spawn_result` follows; recovery lists intents without results as `NeedsReconciliation` and never respawns. No general effect-executor framework |
| R4 | §6.2 dropped critical `Abstain` from the decided set, so one unavailable critical judge plus a passing item could `Pass` | plan §6.2 rule 3 | **accepted.** Any critical `Abstain` → `Abstain`; coverage (`decided_weight / total_weight`) is a separate gate with default 1000 |
| R5 | the re-read at §6.3 step 5 checks legality, not identity; an old attempt's verdict can land on a new attempt | plan §6.3 | **accepted.** A `VerificationToken { plan, version, task, attempt, contract_digest, criteria_digest, output_digest, snapshot }` is committed at step 2 and compared whole at step 5; the snapshot id is the shadow-gitdir tree the turn checkpoints already capture (`checkpoint.rs:16-27`) |
| R6 | `Wall::check` is cooperative, not a sandbox; `deny_read` is paths, `deny_url` is prefixes, so the judge's `history://` block must be `deny_url` | `wall.rs:114-115`, `wall.rs:10-14,60-64` | **accepted.** §6.4 uses `deny_url`; the plan states the cooperative limit wherever it says "walled" |
| R7 | `take_settled_worktree` takes the handle on merge and discard; `step_todo` reaps on every exit from Running, so a blanket "refuse reap of an unmerged worktree" blocks fail/drop/supersede cleanup | `lane/mod.rs:1033-1052`, `ops.rs:699-708` | **accepted.** The guard is on `done` only (verified candidate, recorded integration); non-success exits record a disposition (Retained, Discarded, MergeFailed) and never merge to free a slot |
| R8 | `take_pending` drains shared counters, so two waiters steal each other's updates; `wake_idle_hook` documents the AgentEnd race a status-gated hook loses | `mailbox.rs:304-319`, `session/hooks.rs:82-96` | **accepted.** The child host's `notice` wires `wake_idle_hook`, not `heartbeat_hook`; `wait` takes a per-caller cursor; the race tests the reviewer lists are F0a's |
| R9 | the probe loop sleeps up to 60 s and only then blocks on the tick; a due time registered mid-sleep is not seen, so "a grace fires within a second" is false | `probe.rs:233-241,246-258` | **accepted.** A `tokio::sync::Notify` wakes the loop on a new earlier due time; the stated latency bound becomes "observed and reported", never inferred |
| R10 | `rlm.put` requires a file-safe token and `family://` serves the metadata sidecar, so `put(label)` and `done(output="family://<label>")` are wrong twice | `rlm/__init__.py:535-536`, `schemes.rs:236-254` | **accepted.** Inline outputs get an artifact id (`<plan>/<task>/<attempt>` or a content digest) and a product resolver serves bytes; the sidecar stays a coordination view |
| R11 | `schema.rs` ignores unsupported keywords; `OutputResolve` returns `Ok(None)` for resolved-but-unserved; neither may become an implicit pass | `schema.rs:3-4`, `ops.rs:259-266`; **worse than stated**: `do_done` validates only `if let (Some(product), Some(document))` (`ops.rs:836-838`), so an unserved product or schema passes today, and a missing `output_resolve` skips validation entirely (:822) | **accepted.** Unknown assertion keywords in a criterion schema are refused; an unserved product or schema is a refusal, never `Pass`; F0c's first fixture pins the current implicit pass as red |
| R12 | 48 levers with a 40-row switch is underdetermined for OLS; the handwritten GP is unjustified | plan §10.1, §10.4 | **accepted.** At most three to five knobs tied to observed failures; controlled comparisons or a bounded grid first; no GP |
| R13 | rows 0022 and 0024 both read 21/21 while cost went $0.0524 → $0.1256 and wall 907.8 → 2724.2 s: activation is not value | `docs/eval-ledger.md:60,62` | **accepted.** §10 gains a value gate (independent grading, complete cost accounting, an overhead tolerance the owner sets) beside the activation gate; F0e records a decision, not a green light for F1-F4 |
| S1 | authority moves to `<runtime-state>/plan-roots/<root-id>/{journal.jsonl,state.json}` outside the checkout; `.yi/plans/<slug>/plan.json` becomes an export | design choice | **rejected as stated; protocol accepted.** The journal protocol (commit record, flush, checkpoint after, projections lag) lands, but the journal stays `.yi/plans/<slug>/ops.jsonl` and the checkpoint `plan.json`, in the one directory D97 tracks and this plan already chose. Relocation neither fixes "a clone of exports is a detached view" (true either way) nor earns a second store |
| S2 | CLI argv is not human authority; administrative ops (fuse reset, repair resolution, legacy acceptance) need host-validated user authorization | the agent's `bash` tool can run `yi plan fuse reset` | **accepted.** The hole was real. Admin verbs submit a request the running session confirms through its existing permission path (`Decision::Ask`, `gate.rs`); the console cites the user's own message; an agent-invoked CLI keeps the agent principal. `Actor::User` is minted only by that path |
| S3 | lossless migration; never delete or truncate the format-1 file on read; keep the reader through a deprecation window | design choice | **accepted with one change.** Migration is an explicit `import` op; the original bytes are kept as an immutable artifact blob under `.yi/plans/<slug>/artifacts/` (not a Markdown document the store reads), notes over the cap become artifact references; the reader stays two releases |
| S4 | ship one shape (`fork_join`) first; every other shape needs a failure-driven proposal | design choice | **accepted, pending the owner (open decision 13.13).** The five other shapes keep their §8.5 specification as prerequisites, not as F1b scope |
| S5 | remove `Plan.replay` (re-executing saved cells is unsafe); `resume` from durable state | plan §8.3 | **accepted.** `program.py` is an audit artifact; `Plan.resume(plan_id)` reattaches, reuses accepted results, exposes uncertain attempts |
| S6 | judges disabled until calibrated; a three-judge panel needs two Pass votes over the requested N | plan §6.4 | **accepted.** F0c refuses new `judge` items until F3; the quorum rule replaces "majority of decided" |
| S7 | reorder F0: transport + wake, then journal + recovery, then verification, then worktree acceptance, then measurement | plan §11 | **accepted.** F0a transport and wake (no storage change), F0b journal and recovery, F0c verification on every path, F0d worktree acceptance and cleanup, F0e measurement |
| S8 | three disjoint task groups (development, validation, sealed final) | plan §10.4 | **accepted as a rule; the sealed set is empty until the task set is larger than thirteen.** Dev and validation now; a final set is added with the fan-out task and any new tasks |
| S9 | the "10% overhead" tolerance and the value gate's decision rule | design choice | **owner's number (open decision 13.14)**; the gate itself is accepted |

Withdrawn from this plan by the disposition: the one-line `heartbeat_hook`
wake fix (replaced by `wake_idle_hook`), `Plan.replay`, `put(label)` as the
inline output path, "a grace fires within a second", the OLS/GP sweep, the
`deny_read` spelling of the judge's history block, the blanket reap guard,
and the `.md` deletion on import. §5-§8, §10 and the F0-F1 stages of §11
are rewritten to the disposition; F2-F4 keep their text and inherit the
rules of §3.6 and §7.

## 1. Context: what Yi is today, in operating-system terms

Yi already has most of a kernel. The Rust runtime (`yi-runtime`) owns a
process table (`crates/runtime/src/subagent.rs`, `SubagentHost`; the family
view in `crates/runtime/src/family.rs`), a step table for todos
(`crates/runtime/src/plan/table.rs:38-141`, fourteen legal transitions), an
admission rule (`table.rs:227-237`, width from `ops.rs:315-317`), a virtual
filesystem behind one resolver (`crates/runtime/src/fetch/mod.rs:367`, eight
schemes in `crates/types/src/url.rs:7-17`), walls (`crates/runtime/src/wall.rs`),
a mailbox (`crates/runtime/src/mailbox.rs`), lanes (`crates/runtime/src/lane/mod.rs`),
timers (the probe ladder `crates/runtime/src/plan/probe.rs:14-24`; the heartbeat
scheduler `crates/runtime/src/schedule/mod.rs:14`), and a ledger
(`custom{plan_op}` records, `crates/types/src/plan/ledger.rs:6-28`;
`custom{fetch}`, `crates/types/src/fetch.rs:7-13`). It has a user space: one
IPython kernel per session with the `rlm` shim
(`python/yi_runtime/src/rlm/__init__.py`) whose every verb is a host request
on one registry (`crates/runtime/src/kernel.rs:154`). What it lacks is the
line between them, and four gaps sit on that line:

1. **done is unverified.** `do_done` validates a declared output schema and
   terminal durability, then applies (`crates/runtime/src/plan/ops.rs:803-839`);
   `delegation.accept` is never read there, and no command runs in `ops.rs`
   or `plan/tool.rs`. Commands run in four other places only: the goal's
   complete (`crates/runtime/src/goal/mod.rs:46-53,325-338`), a check-spawned
   child's result (`crates/runtime/src/mailbox.rs:390-400`), discovery
   re-adjudication (`mailbox.rs:432-438`), and the external probe ladder
   (`crates/runtime/src/plan/probe.rs:79,122-123`). `docs/YI_DESIGN.md:1231-1233`
   still says "a done claim is host-verified"; the kernel-side plan skill
   says the same (`python/skills/plan/SKILL.md:3-6`). Both are stale.
2. **a child's lifecycle does not wake its parent.** The terminal notice
   rides `notice_hook`, which queues a steer and starts no turn
   (`crates/runtime/src/session/hooks.rs:172-179`; wired at
   `crates/runtime/src/wiring.rs:603`), while the child-to-parent report
   rides `heartbeat_hook`, which starts a turn when the parent is idle
   (`hooks.rs:57-80`; `wiring.rs:609`); and `wake_idle_hook` (`hooks.rs:82-96`)
   exists because a status-gated hook loses a message that arrives at
   `AgentEnd`, while the status is still Running. Doctrine tells the model to block on
   the child and end the turn (`crates/runtime/src/prompts/doctrine.md:343`),
   so a parent that obeys sleeps until something else wakes it. Nothing
   computes "stuck" on a clock: `family.rs:136` runs at `rlm.status` and at
   environment render only. (§2 rows B7, B3 re-verify the lines.)
3. **plan ops are tool-only.** Mutations reach the engine through the JSON
   tool (`crates/runtime/src/plan/tool.rs:523-527`) and through the probe's
   host-attributed unblock (`probe.rs:190-198`); the only plan host request
   is `plan.get` (`crates/runtime/src/plan/mod.rs:377-392`). The kernel-side
   plan skill calls `plan.create`, `plan.update`, `plan.edit` and
   `plan.split` (`python/skills/plan/src/plan/__init__.py:29,50,66,83`),
   none of which is registered anywhere (`grep 'registry.register("'
   crates/runtime/src`, §2 row A14). A program in the kernel cannot move a
   todo today.
4. **the store is Markdown with hand edits.** A plan is
   `.yi/plans/<id>.md` (`crates/runtime/src/plan/store.rs:214-216`): JSON
   frontmatter between two `---` lines and a free Markdown body
   (`store.rs:259-260`); `PlanFile { plan, body }` (`store.rs:137-141`). The
   engine re-reads the file before every op and diffs it against its last
   snapshot to find hand edits (`store.rs:376-392`; `ops.rs:547-563`), and the
   spawn fuse's only way down is "a user editing the plan file"
   (`crates/types/src/plan/doc.rs:463-464`). The user's authority is a text
   editor, which no op record can cite.

Everything below closes those four gaps and then builds the layers that
become possible once they are closed: contracts, a mailbox with envelopes and
leases, plans as programs, a procedural graph, and a sweep that tunes the
kernel's constants from the ledger. The principle, settled in the design
sessions and unchanged here: **models write policy and content; the kernel
enforces mechanism; triggers read data.**

## 2. Findings the code settles

Each anchor handed to this plan, re-verified against the tree at 2d1d7aae.
"matches" means the line and the claim both hold; otherwise the tree wins and
the row says how. Rows are grouped by the recon that produced them (A: plan
engine and types; B: process, mailbox, hooks, lanes, goal, prompts, Python;
C: docs and governance).

| id | anchor as handed in | found | verdict |
|---|---|---|---|
| A1 | `do_done` validates output schema and terminal durability only, never runs the acceptance (`ops.rs:803`) | `crates/runtime/src/plan/ops.rs:803-839`: `locate_step` (:809), declared output schema (:810-838), `check_terminal` (:839); no `run_check`, `Command::new`, `sh -c` in `ops.rs` or `tool.rs`; `run_check` is `goal/mod.rs:46`, reached from `probe.rs:79`, `mailbox.rs:396,436` and `goal/mod.rs:329` | matches |
| A2 | `Actor` enum Owner/Child/User(Url)/Host (`ops.rs:24`) | `ops.rs:23-29` verbatim | matches |
| A3 | `fold_user_edits` reaps children a hand edit displaced (`ops.rs:547`) | `ops.rs:547-563`; one caller `ops.rs:515`; `store.user_edits` at `store.rs:376`, one production caller `ops.rs:555`; `HandEdit { left_running }` `store.rs:143-146` | matches |
| A4 | `apply` takes the store lease (`ops.rs:399`) | `apply` at `ops.rs:395`; `check_actor` :398; lease :399, RAII for the whole op | line off by 4 |
| A5 | width `clamp(cores-1, 1, 8)` (`ops.rs:315`) | `ops.rs:315-317`: `cores.get().saturating_sub(1).clamp(1, 8)` | matches |
| A6 | `do_start` inline `by: main` vs delegated spawn (`ops.rs:774`, 789-792) | `do_start` at `ops.rs:759`; the branch at :774-776 (`OWNER_AGENT` = "main", `ops.rs:18`); fuse charged on the root :780-787; spawn :789-792 | function starts at 759 |
| A7 | `reap_leaving_running` 717, `do_retry` 866, `do_supersede` 971, `resolve` 604 | 717, 866, 971 match; `resolve` is `ops.rs:606` (a second `fn resolve`, `OutputResolve`, sits at :262) | one off by 2 |
| A8 | STEPS table (`table.rs:37`); decompose legal on any Running todo (:77) | `Step` :31-36; STEPS :38-141, fourteen rows; Decompose row :74-78. The table alone allows it; `do_decompose` adds `DepthExhausted` unless Root (`ops.rs:901-909`) and `PlanExists` (:918-922) | table at 38; claim needs the two refusals |
| A9 | `Actor::User` may only Unblock or View (`table.rs:157`) | `check_actor` at `table.rs:159-164`: `Actor::User(_) \| Actor::Host => Unblock \| View`; `Actor::Child => View` | line 159; Host shares the arm |
| A10 | spawn fuse 64 (`ids.rs:340`), retry cap 8 (`table.rs:187`) | `SPAWN_CAP` `crates/types/src/plan/ids.rs:340`; `charge_spawn` `table.rs:172-175` (called `ops.rs:783,786`); `RETRY_CAP` `table.rs:186`; `charge_retry` :188 refuses at `>=` | retry cap at 186 |
| A11 | `admissible` takes the first N ready todos in Vec order (`table.rs:262`) | `table.rs:227-237`: undelegated ready todos are admitted unconditionally and take no slot; only delegated ones draw from `slots`; order is `Plan::ready()` Vec order (`doc.rs:583`) | **claim wrong**: not first-N; inline todos bypass slots |
| A12 | `canonical_plan` 69, `DEFAULT_STALE_TURNS` 12 (:49), `plan_json` 220, stale reminder 306 (`plan/mod.rs`) | all four match; `PLANS_DIR = ".yi/plans"` at `mod.rs:51`; `plan.get` registered :377-392; `attach_plan` :396-422 delivers through `session.heartbeat_hook()` (:401) | matches |
| A13 | probe ladder 60 s doubling to 30 min (`probe.rs:14-17`), tick 105, run at 123 | `FIRST_DELAY` :14, `MAX_DELAY` :17, `IDLE_POLL` 60 s :21, `PROBE_TIMEOUT_MS` 30 s :23, `SATURATED_SHIFT` 5 :24; doubling `60 << rung` :36; `tick` :105; run :122-123 through `goal::run_check` (`sh -c`, no cwd: `goal/mod.rs:47-48,53`) | matches |
| A14 | mutating ops reach the engine only through the JSON tool (`tool.rs:523`); `plan.get` the only plan host request | `tool.rs:523-527` and **also** `probe.rs:198` (`Op::Unblock` as `Actor::Host`); `plan.get` is the only registration (`mod.rs:379`); `python/skills/plan/src/plan/__init__.py:29,50,66,83` call four unregistered names | second production path; dangling skill |
| A15 | `PlanOpRecord` and `SessionOpSink` (`ledger.rs:9`), report 88-101/173, lint six rules called by `yi plan lint` and one test | `SessionOpSink(pub StoreHandle)` `plan/ledger.rs:9-25`; `OpSink` trait is `ops.rs:255-257`; `PlanOpRecord` is `crates/types/src/plan/ledger.rs:12-28`; `Report` struct :76-83, `report()` :173, `serial_fraction` :88, `discovery_ratio` :98; `lint` :248-334, rules `width`, `ephemeral-terminal`, `unknown-state`, `unrunnable-acceptance`, `no-probe`, `unknown-blocker`; callers `crates/cli/src/plan.rs:76`, `crates/runtime/tests/plan_ledger.rs:263` | record and trait live in other files |
| A16 | `kwargs_of` passes name/model/thinking/isolation/check only (`dispatch.rs:84`); wake test 538-590 | `dispatch.rs:84-107`, five keys (:86,88,91,96,105), `Isolation::Other` refused :98-102; `spawn` :132-135; `reap` :139, relevance measured :146-149 before `host.reap` :150; test `the_follow_up_wakes_an_idle_owner_and_names_the_held_count` :537-590 | matches |
| A17 | gate consts 15-26; `stop_posture` Quiet on `Running{child}` (:121) | `pub mod gate` :17, consts :18-26; `stop_posture` :105; the `Running` arm :121-125 latches only for the owner; Continue iff ready non-empty, blocked on child, running inline, or failed (:134-137), else Quiet | **claim wrong**: a running child sets nothing; Blocked{Child} is Continue |
| A18 | `Check` (`doc.rs:60`), `BlockedOn`, `TodoState` 85, `clears_edge` 119, `SpawnSpec` 165 without wall, `Delegation` 191-205, `Plan` 455, fuse comment 463, `ready` 583, `finished` 598, `validate` 604 | `Check` :60-65; `BlockedOn` :71-79 (`External { probe }`); `TodoState` :85; `clears_edge` :119-121 (Failed does not clear); `Isolation` :152; `SpawnSpec` :165-179, no wall field; `Delegation` :194-205 (`spec, accept, output, context, note, extra`); `Plan` :457-469, comment :463-464, private `spawns` :465; `ready` :583, `finished` :598, `validate` :604 | two structs two lines later |
| A19 | `Spawns`, `SPAWN_CAP`, `RetryCount` (`ids.rs`) | `Spawns` :322, `SPAWN_CAP` :340, `RetryCount(pub u8)` :343, `TouchCount` :311, `PLAN_FORMAT: u32 = 1` :5, label 80 / goal 512 / slug 40 / id 96 :6-9, `INLINE_NOTE_MAX_BYTES` 1024 :12 | matches |
| A20 | `crates/types/src/plan/ledger.rs` holds the op ledger types | 28 lines: `PLAN_OP_ENTRY_TYPE = "plan_op"` :6; `PlanOpRecord { plan, op, actor, at, todo, from, to, todos, extra }` :12-28 | matches |
| A21 | D77 ladder fields on the legacy `Task` (`plan.rs:60-71`), nothing reads them | `crates/types/src/plan.rs:63,67,71`; readers outside the definition: `crates/types/tests/wire_roundtrip.rs:136-147,209-233` and fixture `crates/types/tests/fixtures/v4-plan-readmit.jsonl` only | matches |
| A22 | `rlm.maxDepth` default 1 ceiling 3 (`config.rs:60`) | `crates/types/src/config.rs:47-61`: doc :47, `depth()` clamp :60 | matches |
| A23 | store is `.yi/plans/<id>.md`, JSON frontmatter plus Markdown body | `store.rs:214-216` path; write :255-273 (frontmatter+body :260, tmp+rename :261-264); `render` :236 = `to_string_pretty` under `FRONTMATTER_CAP_BYTES` 32 KiB :9; read :218 via `parse_document` :106 and `split_frontmatter` :74; `PlanRepr` `doc.rs:481-499` (`format` pinned to 1, `DocError::Format` :527-532); `TodoRepr` :315-340 (nested `children`) | matches |
| A24 | the store lease | a lock directory `.yi/plans/.lease/` (`store.rs:11`), `LEASE_ATTEMPTS` 8 :14, `lease()` :320, hold file named per holder :162-163, stale by pid :347-348 or 30 s mtime :318-319, RAII release :186 | verified |
| A25 | `Op` vocabulary | `ops.rs:45-100`: Init, Append, Drop, Block, Unblock, Reorder, AddEdge, Start, Done, Fail, Retry, Decompose, Supersede, Set, View; `OpKind` `table.rs:12-29`; wire names `table.rs:138-155` | verified |
| A26 | walkthrough fixtures | three files under `crates/runtime/tests/fixtures/plans/`; consumers `plan_walkthrough.rs:43,927-959`, `loop_coupling.rs:252-270` (hard count 3 at :269), `tool.rs:617` | verified |
| A27 | `yi plan` verbs | `lint` and `report` only (`crates/cli/src/plan.rs:17-27`); no mutating verb; console rpc `plan` is read-only (`crates/cli/src/rpc.rs:217-230`) | verified |
| A28 | the plan tool's actor | fixed at construction (`tool.rs:515-520`), never read from arguments (:344-348) | verified; authority is a code path |
| C1 | tree version and last D-row | `docs/ARCHITECTURE.md:4` 0.207.0; decision log header :141-144; D171 at :145; D169/D170 absent from the log and `docs/solutions/adr/` | verified |
| C2 | D-rows to amend or carry | D26 :294, D53 :258, D77 :234, D85 :226 (tiers), D97 :215, D105 :209, D133 :181, D137 :177, D160 :154, D161 :153, D164 :150, D165 :149, D166 :148 | verified |
| C3 | changelog row shape | three columns `ver, date, change` (`docs/CHANGELOG.md:6-7`); D-refs, `Closes #N`, and the `growth +N:` memo ride inside the change cell; top row 0.207.0 :8 | verified |
| C4 | row 0025 reads `children_spawned` 0 (`docs/eval-ledger.md:63`) | line 63, but as prose in the notes cell ("`children_spawned` 0"), not a column; the table has 18 columns (:37-38); rows 0026/0027 do not exist in this tree | column claim corrected |
| C5 | `orchestrate.md` attached 158/158 | `docs/plans/2026-09-08-pass-levers.md:51` verbatim | matches |
| C6 | YI_DESIGN says done is host-verified | `docs/YI_DESIGN.md:1231-1233` and :1220-1222 (§8.17.1 at :1211); §8.10 at :869; §8.17 at :1195 | matches, stale |
| C7 | guardrails | file 1,200 (`scripts/guardrails/check_file_size.py:11-12`), function 150 (`check_fn_size.py:8`), growth memo in the version's changelog row past +150 and a D cite past +2000 (`check_growth.py:2-5,13`; `_common.py:7`), ratchets are one JSON each under `scripts/guardrails/baselines/` and shrink only (`.ruler/040-guardrails.md:6-8`), schemas lock (`check_schemas_lock.py`), env surface 6 of 40 (`baselines/env_vars.json`; `check_env_surface.py:12-13`), test tiers marker (`check_test_tiers.py:11`), subject 72 (`check_commit_style.py:24`), PR metadata gate (`check_pr_metadata.py:8-16,300-316`) | verified |
| C8 | event vocabulary cap | design-time only: `docs/YI_DESIGN.md:1260` (Event ≤ 13) and :1282-1284 (≤ 18 across layers); no script counts it; `AgentEvent` has fourteen variants today (`crates/types/src/event.rs:216-279`) | no enforcing gate; the rule here is "add none" |
| C9 | one issue per stage, one row and one ADR per PR | issue and row: `.ruler/095-tracking.md:3-16`, `.ruler/090-workflow.md:34-38`, `check_pr_metadata.py`; ADR: `.ruler/090-workflow.md:41-42`, `just adr` (`justfile:251-253`, `scripts/adr.py`); template `docs/solutions/adr/d166.md` (`# D<n>: title`, `Status`, `## Decision`, `## Why`, `## Reversible via`); stacked PRs undocumented, linear history required (`docs/FORGE.md:188-189`) | verified |
| C10 | held-out split, slice file | none exist; the slice is `TASKS` in `evals/drivers/tbv4_baseline.sh:10`, dataset digest :9, one-hour ceiling :13-15; `evals/` is stdlib-only (`evals/README.md:3-5`) | absent; §10 creates both |
| C11 | `.yi/schemas`, `help(yi)` | neither exists (`.yi/` holds `mining/` only; no `help(` in `python/yi_runtime/src/rlm/`) | absent; §5 and §8 create them |
| C12 | the kernel-side Python packages | `python/yi_runtime/pyproject.toml:16-17` packages `src/rlm`; four skills installed by `PYTHON_SKILLS` (`crates/kernel/src/bootstrap.rs:270-275`): compact, attach_image, goal, plan; the gate runs `python/yi_runtime/tests` (`check_guardrails.sh:36`) | verified |
| C13 | test tiers | D85 (`docs/ARCHITECTURE.md:226`): T0 unit, T1 faux cassettes (both `just check`), T2 real-binary journeys (`just journeys`, `justfile:115-128`, marker exact), T3 paid smoke (user-run, ledgered); T3-live added by D133 (:181) | verified |
| M1 | `yi why` and the commit trailer | `crates/runtime/src/plan/why.rs:11` (`Plan: plan://`), `answer` :91, `commits_for` :129; CLI `crates/cli/src/why.rs:14` | exists (the blame view) |
| M2 | fetch log and pins | `crates/runtime/src/fetch/log.rs`: `record` :39, `relevance` :74, `backs` :82, `unbacked` :87, `register_pin` :95, `rewrite_terminal` :122, `relevance_of` over a transcript :167 | exists (quote verification rides it) |
| M3 | the session store's custom entries | `append_custom(lane, custom_type, data)` `crates/session/src/store.rs:185-190`; `EntryQuery { entry_type, custom_type, order, limit, after_seq }` `crates/session/src/query.rs:12-18`; `Entry::Custom` `crates/types/src/entry.rs:76` | exists (the durable inbox rides it) |
| M4 | `AgentMessage::Custom` carries `details` | `crates/types/src/message.rs:237`; constructed with `custom_type, content, display, details, timestamp` at `plan/mod.rs:364-370` | exists (the envelope rides it) |
| M5 | the host registry | `HostRegistry::register(name, handler)` `crates/runtime/src/kernel.rs:154`; handler returns `Result<Map<String, Value>, String>` (`plan/mod.rs:379-391`); registrations today: `rlm.*` (`subagent.rs:899-1010`), `agent_message.*` (`mailbox.rs:102-115`), `goal.*` (`goal/mod.rs:562-583`), `plan.get`, `fetch` (`wiring.rs:209`), `history.grep` (:256), `compact.*` (:471-485), `rlm_heartbeat.*` (`schedule/mod.rs:1010-1121`), `exec.*` (`kernel.rs:165-238`), `rlm.find_models` and `model.info` (`subagent.rs:1012,1027`), `rlm.merge_worktree`/`rlm.discard_worktree` (:991-1010) | verified |
| M6 | the JSON Schema validator | `crates/runtime/src/schema.rs`: a `type/required/properties/items/enum` subset, errors carry the JSON path (`check(.., "$", ..)` :41) | exists (plan.json validation rides it) |
| M7 | the fuzz lane | `crates/runtime/tests/plan_fuzz.rs` drives `PlanEngine` over a temp `PlanStore` with proptest, 256 cases (:29) | exists; grows with the new ops |
| B1 | caps: depth default 1, 8 children per parent (zombie comment :16), 16 per family (`subagent.rs:18,20`) | `DEFAULT_MAX_DEPTH` :15, comment :16-17, `DEFAULT_MAX_CHILDREN` :18, `FAMILY_CAP` :20 (enforced :492-495); `spawn` :470-474; `run_child` :621-627; terminal notice `(self.options.notice)(&notice)` :715; `delete` :846, its worktree guard :851-858; `register` :897; the wall built from kwargs at :486 with the allowlist `name model thinking fork isolation deny_write deny_read deny_url context check` :273-282 | guard is 851-858 |
| B2 | mailbox constants and paths (`mailbox.rs:10-20, 154, 217, 286, 338, 396, 415, 436, 491`) | `WAIT_MIN_MS` 1 s :10, `WAIT_MAX_MS` 300 s :11, poll 100 ms :12, `CONTEXT_MAX_KEYS` 8 :14, value cap 4,096 :15, total cap 16,384 :16, `RESULT_TAIL_CHARS` 2,000 :17, `MAX_DISCOVERIES` 16 :20; `route(from, target, text, followup)` :154-160; `deliver_to_parent` :217-226 (bumps `pending`, calls `options.report`); `wait` :286-292 drains via `take_pending` :304-319; `result` :338-342, the check-spawned child's `run_check` :390-400; `route_discoveries` :415, re-adjudication `run_check` :432-438; `reap` :491-500 removes the record with its `Option<Lane>` and no settle check (no `worktree`, `Lane` or `lane` word in the file outside a test fixture at :660) | matches; reap has no guard |
| B3 | stuck computed only at `rlm.status` or environment render, `STUCK_IDLE_MS` 300 s (`family.rs:9, 136`) | `STUCK_IDLE_MS` :9; `state_from_records` :136-160 (idle test :155-156); its one caller `subagent.rs:732` inside `states()` :719, whose two callers are `status()` :759-761 (registered `rlm.status` :975) and `environment.rs:229` | matches |
| B4 | walls exist only as `rlm.run` kwargs (`wall.rs:60`) | `Wall { deny_write, deny_read, deny_url }` `wall.rs:10-14`; `from_kwargs` :60-64, called `subagent.rs:486`; enforced at the tool seam `tools.rs:231` (`Wall::check` :115), the resolver `wiring.rs:193` (`check_url` :74), and read paths `tools.rs:199` (`check_read_path` :102); `session.set_wall` `session.rs:333` | matches |
| B5 | `next:` lines are hand-maintained strings (`todo/tool.rs:12`; `affordance.rs`) | `NEXT = "next: "` `affordance.rs:7`; nine producers :9-84 (`spawned`, `coroutine_leak`, `method_awaited`, `listing_name`, `child_finished`, `grid_empty`, `compacted`, `call_template` the one derived from a tool schema :57, `append` :84); call sites `subagent.rs:607,711`, `tools.rs:67-84,182,305-308`, `wiring.rs:663`; a second producer `todo/text.rs:204` (`moves`) and :218 (`next_lines`, cap `NEXT_LINES` 3 at :6); `todo/tool.rs:12` DESCRIPTION 1,013 chars ending "Every result ends with next: lines you can copy." | matches |
| B6 | `notice: session.notice_hook()` (`wiring.rs:603`); report uses `heartbeat_hook` (:609) | :603 verbatim; `report` :609-614 wraps `session.heartbeat_hook()` with `DeliveryMode::Steer`; registry built :465-467, handed to the kernel service :532; registrations by module: mcp stubs and exec (`kernel.rs:241-242,167-238`), compact (:471,485), subagent (:501), the child link's `agent_message.*` overwrite (:508, `mailbox.rs:102,115`), schedule (:510), goal and plan (:511), fetch (:520), history (:248-256) | matches |
| B7 | `heartbeat_hook` starts a turn when idle (`hooks.rs:57-79`); `notice_hook` queues only (:172-179) | `heartbeat_hook` :57-80: running → push on `steer` or `follow_up` (:68-75), idle → `run(message)` (:77); `notice_hook` :172-179 pushes `user_message(text)` on `steer` and starts nothing; `session.rs:710` `follow_up_message` ("queued for the next turn, never starting one"), :718 `deliver` = heartbeat Steer | matches |
| B8 | lane pool: 3 slots per repo shared with roots (`lane/mod.rs:17, 461`); `Drop for Lane` keeps the branch only if merged (:879, 845) | `DEFAULT_SLOTS` :17; `PoolFull` :193-194; `Pool::open` keyed by the common git dir :461-477 (`~/.yi/lanes/<short>`); `claim` :659; `detach` :845-859 deletes the branch only when merged; `Drop` :879-892 detaches; `merge_into` :940-955 commits uncommitted work first (incident note :939); `merge_worktree` :1013, `discard_worktree` :1024, `take_settled_worktree` :1033-1052 refuses while Running | merge_into runs to 955 |
| B9 | `run_check` `sh -c` with no cwd (`goal/mod.rs:46`); complete runs the check (:305) | `run_check` :46-53 (`sh -c` :47-48; `run_captured(command, None, ..)` :53; `DEFAULT_CHECK_TIMEOUT_MS` 600 s :23, `DISCOVERY_CHECK_TIMEOUT_MS` 60 s :87); `update` :305-342: drain gate :316-324, then the check :325-338, then the write; `drain` :347; `block_after_error` :506-513; `Aborted` deferral :549; `goal.get/create/update` registered :562-583 | matches |
| B10 | route prefilter scores ≥ 4 Complex (`ext/orchestrate.rs:89`); features :95-128 | `route` :86-90 (≤ −3 OneShot, ≥ 4 Complex); `features(prompt, repo_dirty, named_paths)` :95-118: short-and-quiet −3, fenced −1, `and` count (cap 3), enumerations, imperatives, questions (−), paths beyond one; post-hoc `edit_before_read` :217, `files_matched` :259-260 (> 5), `failed_check_after_edit` :262-263, `tool_calls_per_turn` :270-271 (> 4) | matches |
| B11 | the todo tool's stop posture is Quiet while blocked on a child (`todo/coupling.rs:340`) | `stop_posture(list, children_running)` :330-341: Quiet whenever `children_running` (:331-333), Ask on `Blocked{User}` :339, Quiet on `Blocked{Child}` :340. This is the todo tool; the plan's `loop_coupling.rs` is row A17 | matches (two modules, two rules) |
| B12 | doctrine tells the model to block on the child and end the turn (`doctrine.md:343`) | rule 4 :343-346 ("you step your todo, blocked `on child` while it runs"); Working model :312-367; Planning :247-263; `orchestrate.md` :26, :28, :32, :51-55, :138 all verbatim; sizes: doctrine 21,709 B, orchestrate 8,131 B, auto_review 1,227 B | Working model runs to 367 |
| B13 | the auto-review envelope | `auto_review.md:19-30` ("Any other output is read as a denial" :30); `REVIEW_TIMEOUT` 30 s `auto_review.rs:15`; `parse_outcome` :62-81 fail-closed; the reviewer runs in its own session with no tools :85-87 | matches; the judge template |
| B14 | the rlm surface and `handle.result` polling | `host_request` :144 over a Jupyter comm `host.request` (:29, :199; reply `status: ok|error` :164-196); `run` :237, `find_models` :276, `list_subagents` :320, `delete_subagent` :329, `send(target, message, followup=False)` :343, `followup` :358, `status` :363, `list_agents` :381, `wait(timeout=300.0)` :390, `interrupt` :399, `result(target, *, schema)` :404, `merge_worktree` :426, `discard_worktree` :435, `fetch(url, *, as_text=False)` :466, `put` :527, `get` :557, `ls` :568 (sync), `bash` :692; `RLMSpawnHandle` :38-43, `result()` :52-83 polls `list_subagents` every 0.5 s, 900 s default; the `rlm` façade :755-819; `__all__` :830-860 omits `status`, `put`, `get`, `ls` | matches; note the `__all__` gap |
| B15 | `children_spawned` counts `rlm.run(` in kernel cells (`extract.py:95`) | `SIGNAL_NAMES` :257-269 (46 names); the count is at :389-397 (`"rlm.run(" in code or "rlm(" in code`; `readers_spawned` on the substring `deny_write`); a plan-dispatched child (spawned by the engine, not by a cell) is invisible to it | line 389; the signal misses engine spawns (§10.6 fixes) |
| B16 | the scheme table (`fetch/schemes.rs`) | dispatch is `fetch/mod.rs:393-408`: local :394 (`resolve_local` schemes.rs:88), kernel :395 (:332; `dump_kernel` :306), plan :396 (:137), agent :397 (:365), history :398 (:179), checkpoint :399 (:438), mcp :400 (:415), user :401 (:386), family :402 (:237), tree :403 (:263), else `FetchError::External` :404; `family` and `tree` are `Scheme::External(..)` special cases, not `Scheme` variants (`url.rs:7-17`) | two schemes ride External |
| B17 | `rlm.maxDepth` (`config.rs:60`) | `RlmConfig { max_depth }` :53-55, `depth()` :59-61, wire key `rlm.maxDepth` (:51) | matches |
| B18 | `AgentEvent::Custom { details }` and a vocabulary-cap test | `AgentEvent` has no `Custom` variant (`event.rs:216-279`, fourteen variants); the envelope carrier is `AgentMessage::Custom { custom_type, content, display, details, timestamp }` (`message.rs:237-244`), persisted as `Entry::Custom` (`entry.rs:76`); producers today: `agent_message` (`mailbox.rs:65`), `discovery` (:468), `reap` (:525), `reminder` (`rules.rs:576`), checkpoints (`checkpoint.rs:103`); no test caps the vocabulary | corrected: the message, not the event |
| B19 | `check_prompt_examples.py` and the request-prefix gate | `check_prompt_examples.py` checks await-ness of coroutine calls; its module prefixes derive from `python/yi_runtime/src/rlm/__init__.py` and `python/skills/*/src/*/__init__.py` (:11-15), over `prompts/*.md` and every `SKILL.md` (:19-21); the byte budget is `check_request_budget.py` over the `request_budget` test; the **name allowlist** is `crates/runtime/tests/ext_e2e.rs:332-367` (`fragment_examples_name_real_kernel_apis`, prefixes `rlm.` and `goal.` :350-359, files orchestrate/identity/doctrine :352-356) | three gates, not one |
| B20 | the host registry | `HostHandlerFn` `kernel.rs:144`; `HostRegistry { handlers, handles }` :148-151; `register(request_type, handler)` :154-161 is a `HashMap` insert, so a later registration of the same name overwrites (how `mailbox.rs:102` shadows `subagent.rs:899` for a child, `wiring.rs:508`); `HostFuture = Pin<Box<dyn Future<Output = Result<Map, String>>>>` `crates/kernel/src/client.rs:65-66`; `HostHandlers::dispatch` :70-76, implemented `kernel.rs:288` | verified |
| B21 | file sizes against the 1,200 cap | `todo/coupling.rs` 1,185; `session.rs` 1,151; `kernel.rs` 1,113; `ops.rs` 1,096; `lane/mod.rs` 1,053; `subagent.rs` 1,049; `schemes.rs` 1,032; `mailbox.rs` 792; `wiring.rs` 686; `goal/mod.rs` 634; `store.rs` 831; `tool.rs` 744 | six files within 150 lines of the cap: new code goes in new files (§11) |

### 2.1 Re-anchoring at ee336f6f (2026-09-13)

The worktree moved from 2d1d7aae to ee336f6f, 394 commits, and the header
version from 0.207.0 to 0.244.0 (docs/ARCHITECTURE.md:4); the last decision
row is D190 (docs/ARCHITECTURE.md:146), and D169, D170 and D171 are now in
the log (:164-166) and under docs/solutions/adr/. Every anchor in §2, §3.7,
§5, §6, §7.5 and the F0 stage descriptions was re-read against this tree;
where the plan and the tree disagree, the tree wins and the rows below say
how. Most drift is a line shift with the claim intact; the rows marked
changed alter a number, a type or an assumption the plan leaned on, and the
Stage impacts list carries each one into the stage that must absorb it.
Rows not listed here (the majority) were re-verified exact and keep their §2
cites.

| id | plan said | tree at ee336f6f | verdict | consequence |
|---|---|---|---|---|
| A1b, A8b, 5.1.ops_child, 5.1.ops_kin, 6.3-ops-810-838, 6.3-ops-836-838 | `crates/runtime/src/plan/ops.rs`: `check_terminal` :839 and schema block :810-838; `do_decompose` refusals :901-909 and :918-922; `PlanId::child` :927; kinship :973-976; `validate_product` hoisted from :810-838; implicit pass :836-838 | `ops.rs` is byte-identical to 2d1d7aae; the plan miscounted: `check_terminal(&label, output.as_ref())?` :845, schema block :810-844, `fn validate_product` :288-313, the `(Some, Some)` skip :838-840 (:836-837 is the `UnusableSchema` arm), `do_decompose` :897 with `DepthExhausted` :903-910 and `PlanExists` :920-924, `allocate_child(&file.plan.id.child(..))` :925, prefix kinship :979-982 | moved | §6.3 and F0c cite :810-844, :288-313 and :838-840; the `unserved-output-today-passes.json` fixture pins :838-840 |
| A25 | `Op` :45-100; `OpKind` `table.rs:12-29`; wire names :138-155 | `Op` `ops.rs:46-102` (same fifteen variants, same order); `OpKind` `table.rs:13-29`; `op_name` :139-157 | moved | none beyond the cites |
| A3b, A23, A24, 5.1.store_lease_fn, 5.5.*, 5.6.user_edits, 3.7-store-324-lease | `crates/runtime/src/plan/store.rs`: `user_edits` :376; path :214-216, write :255-273, render :236, read :218, `parse_document` :106, `split_frontmatter` :74, `DocumentError` :65; `lease()` :320-348 with pid :347-348 and mtime :318-319, RAII :186; `create_dir` :324 | `lease()` :321-364 now delegates staleness to `yi_kernel::bootstrap::lock_is_stale` (:342) and condemns by hold at :347; `create_dir` :325; hold invariant :162-168; mtime doc :319-320; `Drop for Lease` :188-191; `user_edits` :366-386; path :215-217; write :254-270 (frontmatter and body :258, tmp and rename :259-263); `render` :239; read :223; `parse_document` :110; `split_frontmatter` :76; `DocumentError` :67. `FRONTMATTER_CAP_BYTES` :9 and the temp-name nonce :19-26 unmoved | moved | §5.1 says the lease is unchanged; it is, but its staleness rule is now the kernel's `lock_is_stale`, so F0b must not fork a second one. §5.5 cites :76, :110, :67 |
| A22, B17, 5.1.config165 | `crates/types/src/config.rs`: `rlm.maxDepth` doc :47, `depth()` :60, `RlmConfig` :53-55, plans dir doc :165 | doc :45; `RlmConfig` :94-105 with `max_depth: Option<u8>` :97; `depth()` :102-103 (`unwrap_or(1).clamp(1, 3)`); `plans.dir` doc :167; `plans` field :32 exact; new `ConfigMigration::RemovedGates` :52-55 (D182) | moved | §5.1 cites :167; the config migration enum is the precedent F0b's `plans.dir` relocation follows |
| M4, B18b | `AgentMessage::Custom` `message.rs:237-244` | `crates/types/src/message.rs:241-248`, five fields unchanged, `details` :246 | moved | none |
| M5, B6-hooks, B6-registry, B6-regs, B20c, F0a.files.wiring_notice, F0a.sig.child_link, F0b.files.wiring456, 7.5.wiring_notice_callers, B4-setwall, B5-callsites | `crates/runtime/src/wiring.rs`: fetch :209, `history.grep` :256, compact :471-485; `notice: session.notice_hook()` :603, report :609-614; registry :465-467, service :532; registrations :471,485,501,508,510,511,520; plans dir :456; notice callers :324,525,567,650; `set_wall` call :333; compacted affordance :663 | fetch :217 (passed into `wire_fetch` :536), `history.grep` :271, compact :487-501; `notice: session.notice_hook()` :620, `report` :626-631 (`DeliveryMode::Steer` :629); `HostRegistry::default()` :481, stubs :482, exec :483, `host: Arc::new(registry)` :548; compact :487/:501, subagent :517, `register_child_messaging` :524, schedule :526, goal and plan :527, fetch :531-539; plans dir :469-473 (`wiring.plans_dir = Some(..)` :473); notice callers :339, :541, :584, :667; `session.set_wall` :596; compacted :680 | moved | F0a's rollback line is `wiring.rs:620`; the child-link overwrite is `wiring.rs:524` calling `subagent.rs:899`/`mailbox.rs:102`; F0b's relocation is :469-473 |
| B7-notice, 7.5.notice_hook, B7-session, 7.5.session209 | `hooks.rs:172-179` `notice_hook`; `session.rs:710` `follow_up_message`, :718 `deliver`; :209 `install_extensions` notice | `crates/runtime/src/session/hooks.rs:192-199` (identical body; `follow_up_hook` and `activity_handle` inserted above); `crates/runtime/src/session.rs:752-757` doc, `deliver` :759-763 (heartbeat Steer :762); `let notice = self.notice_hook()` :217; `set_wall` :350 | moved | §7.5 cites :192-199 and :217; `wake_idle_hook` :82-96 unmoved |
| B4-wall, B4-tools, B4-resolver, B4-read, 3.7-wall-100-110, 6.4-wall-13-64, 6.4-wall-114-115 | `wall.rs`: `from_kwargs` :60-64, `check_url` :74, `check_read_path` :102-110, `check` :115, cooperative doc :114-115; enforced at `tools.rs:231`, `wiring.rs:193`, `tools.rs:199` | `crates/runtime/src/wall.rs`: struct :10-14 exact, `parse_prefixes` :36 (boundary rule :34-35), `from_kwargs` :55-61, `check_url` :69, `check_read_path` :97-106 (lexical normalize :98, canonicalize :103), "Not a sandbox" doc :108-109, `check` :110. Tool seam `crates/runtime/src/tools.rs:222`; `check_url` moved out of wiring into the fetch resolver `crates/runtime/src/fetch/mod.rs:388` (second caller `crates/cli/src/catalog.rs:28`); `check_read_path` now on the fetch schemes `crates/runtime/src/fetch/schemes.rs:128,290`, no longer in `tools.rs` | moved | §3.7 and §7.6 cite :97-106 and :108-109; F0c's judge wall is enforced in `fetch/mod.rs:388` and `schemes.rs:128,290`, not the tool file |
| B5-callsites | `call_template` site `tools.rs:182`, `append` sites :305-308 | `crates/runtime/src/tools.rs:173` and :296,:299; :67-84 and `subagent.rs:607,711` exact | moved | none |
| B8-pool, B8-detach, B8-merge, B8-worktree, 6.6-lane-*, F0d-lane-ranges | `lane/mod.rs`: `PoolFull` :193-194, `open` :461-477, `detach` :845-859, `Drop` :879-892, `merge_into` :940-955 (note :939), `merge_worktree` :1013, `discard_worktree` :1024, `take_settled_worktree` :1033-1052, Running refusal :1043-1047 | `crates/runtime/src/lane/mod.rs`: `PoolFull` :194, `open` :471-482 (`~/.yi/lanes/<short>` :477), `detach` :847-861 (`is-ancestor` :856, `branch -D` :857-858), `Drop` :881-894, `merge_into` :942-957 (note :941, `add -A` and commit :945-949), `merge_worktree` :1015-1016, `discard_worktree` :1026-1027, `take_settled_worktree` :1035-1053 (Running refusal :1044-1048, `worktree.take()` :1049-1052); `DEFAULT_SLOTS` :17 and `claim` :659 exact | moved | §6.6 and F0d cite :941-957 and :1013-1053; the `impl SubagentHost` block is :1013-1055 |
| B1-guard, F0d-subagent-851-858 | `subagent.rs:851-858` delete's worktree guard | `crates/runtime/src/subagent.rs:852-859` ("merge or discard it first"); :851 is `let key = …`; `delete` :846 and `register` :897 exact | moved | F0d cites :852-859 |
| B2-fixture | `mailbox.rs:660` the only lane word | `crates/runtime/src/mailbox.rs:647` `lane_slots: 1` fixture; file 779 lines | moved | none |
| B3-env | `environment.rs:229` second `states()` caller | `crates/runtime/src/environment.rs:242` | moved | none |
| B11 | `todo/coupling.rs:330-341` stop posture | `crates/runtime/src/todo/coupling.rs:136-147` (Quiet on `children_running` :137-139, Ask :145, Quiet :146); the file shed 304 lines | moved | none; B21 row below |
| B16 | `fetch/mod.rs:393-408` scheme dispatch | `crates/runtime/src/fetch/mod.rs:394-409`, arms in the same order, `External` :405-408; `schemes.rs` resolvers keep every cited line | moved | none |
| B19d, F0a.files.ext_e2e350, F0a.test.ext_e2e374 | `ext_e2e.rs:332-367` allowlist, prefixes :350-359, files :352-356; wake test :374 | `crates/runtime/tests/ext_e2e.rs:322-361` `fragment_examples_name_real_kernel_apis`, api roster :340-341, prefix check :349 (`rlm.` and `goal.` only); `effects_apply_in_emit_order_and_reminders_reach_the_notice_hook` :364 | moved | F0a and F1a widen :340-341 and :349; F0a's existing-test row cites :364 |
| M6, 5.2.schema41 | `schema.rs:41` the `$` call | `crates/runtime/src/schema.rs:38-39` (`validate` :38-40; `check` def :104); :41 is a brace | moved | §5.2 cites :39 |
| M7 | `plan_fuzz.rs:29` 256 cases | `crates/runtime/tests/plan_fuzz.rs:32` `CASES`, honoured only when `PROPTEST_CASES` is unset :982-983; `MAX_ACTIONS` 20 :33 | moved | none |
| A15-test | lint caller `plan_ledger.rs:263` | `crates/runtime/tests/plan_ledger.rs:252` (file rewritten for the scratch helper) | moved | none |
| A16-test, F0a.test.dispatch537 | wake test `dispatch.rs:537-590`; F0a's sibling `plan_dispatch::…` beside :537 | `the_follow_up_wakes_an_idle_owner_and_names_the_held_count` is an inline `#[cfg(test)]` module in `crates/runtime/src/plan/dispatch.rs:525-577`, on `crate::scratch::Scratch`; no `tests/plan_dispatch.rs` exists | moved | F0a's wake test lives in that src module, named `a_childs_finish_wakes_an_idle_owner` beside :525 |
| A17-posture | `loop_coupling.rs` `stop_posture` :105, Running arm :121-125, Continue :134-137 | `crates/runtime/src/plan/loop_coupling.rs:104`, :120-124, `Blocked::Child` Continue :132 (whole fn shifted by the const deletion in the next row) | moved | none |
| A26 | `tests/loop_coupling.rs:252-270` fixture count | `crates/runtime/src/plan/loop_coupling.rs:252-269` (`assert_eq!(seen, 3)` :267; the test moved into the src module); `plan_walkthrough.rs:47,929-955`; `tool.rs:614` | moved | none |
| C2, C13 | D-rows D26 :294 … D166 :148; D85 :226, D133 :181 | every row shifted +21: D26 :315, D53 :279, D77 :255, D85 :247, D97 :236, D105 :230, D133 :202, D137 :198, D160 :175, D161 :174, D164 :171, D165 :170, D166 :169 | moved | §3.5 amendment cites move with them |
| C7c, C7g, C12c | `check_growth.py:13` `DROW`; `check_env_surface.py:12-13`; `check_guardrails.sh:36` | `DROW = 2000` :14 (docstring :2-9 grew a `--calibrate` paragraph); env cap :13-14; the Python unittest line :37 | moved | none |
| 3.7-cargo-22-published | `Cargo.toml:22` says nothing is published | :22 is `version = "0.2.0"`; the "publishes no crate" comment is :72; no `publish` key anywhere | moved | §3.7 cites :72 |
| 3.7-process-* | `crates/tools/src/process.rs:31,118-124` process group and kill tree; `run_captured` :110-127 without `env_clear` | `kill_tree` :33, `group_kill` :132-144, `process_group(0)` :231 in `run_captured_live` :211-232; `run_captured` :202-209; :110-127 is now `pid_kill`; no `env_clear` anywhere under crates/ | moved | §3.7 cites :33, :132-144, :202-232 |
| 6.3-checkpoint-* | `checkpoint.rs:16-27` shadow-gitdir tree; `capture` :39 | `crates/tools/src/checkpoint.rs:40-46` `struct Checkpoints` doc; `capture` :75 | moved | §6.3 cites :40-46 and :75 |
| F0a.files.rlm_wait, 7.5.ponytail81, B14b | `rlm/__init__.py:390` `wait`; ponytail comment :81; `__all__` :830-860 | `async def wait(timeout=300.0)` :392; comment :83; `__all__` :832-862 still omits `status`, `put`, `get`, `ls` | moved | F0a cites :392 and :83 |
| F0c-tool-344-350-parse_op, 0.241.0 and 0.243.0 | the shared parser is `tool.rs:344`; F0c's unknown-key test verifies `parse_op` at :344-350 | `fn parse_op` `crates/runtime/src/plan/tool.rs:274-342`, called from `fn request(actor, args)` :344-350; `engine.apply` :527; `parse_op` is key-by-key `opt`/`need` with no unknown-key refusal | moved | F0a and F0c cite :274-342; `request` :344 is the actor-fixing wrapper |
| A17-consts | gate consts `loop_coupling.rs:18-26` | `NUDGE_CAP_PER_CYCLE` deleted; `mod gate` :17 holds `STOP_CAP_PER_CYCLE` :18 through `INTERROGATIVES` :23-25 | changed | §10.1's levers manifest lists the constants that exist; the nudge cap is not one |
| B2-reap, F0d-mailbox-491-reap | `reap` :491-500 removes the record with its `Option<Lane>`, no settle check | `pub fn reap(&self, target) -> Result<Harvest, String>` :491-545 promotes the last product before freeing the slot; `struct Harvest` :546; still no worktree or settle guard | changed | F0d's disposition rides on `Harvest`, not on a bare drop; the "no settle check" finding still holds |
| B5-desc | `todo/tool.rs:12` DESCRIPTION 1,013 chars | `crates/runtime/src/todo/tool.rs:13`, 698 chars, same closing sentence; the tool also gained `unleak` :187 for GLM call markup (0.241.0) | changed | §4.4's byte arithmetic for the todo description is 698; `unleak` is the leniency precedent F0a's `plan.op` parser must not undercut |
| B12b | `orchestrate.md` 8,131 B | 8,144 B (`timeout=420` at :75 and :95); doctrine still 21,709 B with `timeout=420` at :356 and :367; every cited line number stable | changed | §11's prompt-byte baselines: doctrine 21,709, orchestrate 8,144, auto_review 1,227 |
| B14, F0a.files.rlm_result | `RLMSpawnHandle.result` polls 0.5 s with a 900 s default, :52-83 | `result` :52-85, default `timeout: float = 540.0` with the D176 comment :56-58, poll 0.5 :59; every name after :53 shifted +2 (`run` :239 … `bash` :694) | changed | F0a keeps 540 s and the comment when it replaces the poll |
| B19c | the byte budget is `check_request_budget.py` alone | still driven by the `request_budget` test, plus a per-tool sha lock in `scripts/guardrails/baselines/tool_surface.json` (`tool:plan` at :24, `tool:todo` :26) since D188; `request_budget.json` reads system 28,331, tools 19,383, total 47,714 | changed | §11 conventions row below (D188) |
| B20d | `HostFuture = Pin<Box<dyn Future<Output = Result<Map, String>>>>` `client.rs:65-66` | `pub type HostReply = Result<Map<String, Value>, String>` :65; `HostFuture = Pin<Box<dyn Future<Output = HostReply> + Send>>` :66; `dispatch` :71 and `kernel.rs:288` unchanged | changed | F0a's `plan.op` handler returns `HostReply` |
| B21 | twelve files against the cap, six within 150 | `todo/coupling.rs` 881, `session.rs` 1,198, `runtime/kernel.rs` 1,127, `ops.rs` 1,096, `lane/mod.rs` 1,055, `subagent.rs` 1,049, `schemes.rs` 1,031, `mailbox.rs` 779, `wiring.rs` 703, `goal/mod.rs` 634, `store.rs` 810, `plan/tool.rs` 744; tree-wide eleven src files sit at or above 1,050 (`crates/tui/src/app.rs` exactly 1,200, `session.rs` 1,198, `kernel/bootstrap.rs` 1,189, `console/app.rs` 1,189, `cli/main.rs` 1,182, `kernel/client.rs` 1,168, `schedule/mod.rs` 1,152, `runtime/kernel.rs` 1,127, `hashline/tool.rs` 1,123, `ops.rs` 1,096, `lane/mod.rs` 1,055) | changed | §11's placement rule stands and now also names `session.rs` (2 lines of headroom), `kernel/client.rs` and `kernel/bootstrap.rs`; `ops.rs` at 1,096 leaves 104 lines for F0b and F0c together, so `verify.rs`, `journal.rs` and `state.rs` are not optional |
| C1, C3 | 0.207.0; D171 last; D169/D170 absent; changelog top row 0.207.0 | 0.244.0 (docs/ARCHITECTURE.md:4, docs/CHANGELOG.md:8); D190 at :146; D169-D171 :164-166 with `docs/solutions/adr/d169.md`, `d170.md`, `d171.md`, `d190.md` | changed | the header `tree:` line is replaced (below); §3.5 drops the D169/D170 absence remark |
| C4 | `docs/eval-ledger.md:63` row 0025, 18 columns :37-38, 0026/0027 absent | row 0025 :66; header :40-41 with 21 columns (`timeouts`, `partials`, `upstreams` added, D174); rows run to 0055 :82; 0026 and 0027 still absent as numbered rows | changed | §10.7's F0e row uses the 21-column header; F0e's row number is the next free one, not 0026 |
| C10 | no slice file; the slice is `TASKS` in `tbv4_baseline.sh:10` | baseline anchors exact, but D186 added `evals/drivers/tbv4_slice.txt` (twelve tasks), `tbv4_sweep.sh`, `tbv4_sweep_tasks.txt`, `watch.py`; no held-out split | changed | §10.6 and §9.6 create only the held-out split; the slice file exists and the F0e run reads it |
| C12b | four skills in `PYTHON_SKILLS` (`bootstrap.rs:270-275`) | five: `[(&str, &str); 5]` :270, `("plan", "plan")` :274, `("memory", "memory")` :275, closes :276 | changed | F0a's deletion makes it `[…; 4]` and removes :274 (the 0.233.0 history row's ":273" is wrong; the tree says :274) |
| F0b.prompt.tool569 | the plan tool DESCRIPTION's sentence about the file goes, about -60 bytes | `crates/runtime/src/plan/tool.rs:569` is byte-identical to the plan base and contains no sentence about a file; the word does not occur in the file | changed | F0b drops the -60 byte line; any DESCRIPTION edit re-locks `tool_surface.json:24` (D188) |
| 3.7-table-163-child-caps | `Child` gets `View` and `Submit` only | `table.rs:163` is `Actor::Child(_) => matches!(op, OpKind::View)`; no `Submit` op exists | changed | `Submit` is an `OpKind` F0c adds, not a right the tree already grants |
| F0d-lane-linecount | `lane/mod.rs` is at 1,053 lines | 1,055 | changed | the argument for `plan/acceptance.rs` stands |
| query.registry | four names the skill calls are unregistered | `git grep registry.register` under crates/ lists no `plan.create`, `plan.update`, `plan.edit`, `plan.split`; only `plan.get` (`plan/mod.rs:379`); `tool.rs:658` is a negative-parse string | gone | §1 gap 3 holds; F0a deletes the skill |
| F0d-dispatch-worktree-settle | `plan/dispatch.rs` (`worktree_state`, `settle`) | neither symbol exists anywhere under crates/; only the `isolation: "worktree"` kwarg at :96 | gone | F0d's Files line labels both as new symbols, not edits |

| decision | what it changed | plan section affected | what this plan now does |
|---|---|---|---|
| D176 / 0.215.0 | the kernel aborts a cell at `cell_ceiling` (`crates/runtime/src/kernel.rs:318`, default 600 s); `RLMSpawnHandle.result` defaults to 540 s with a comment saying why (`python/yi_runtime/src/rlm/__init__.py:56-58`) | §7.5, F0a | the cursored `wait` and the replacement for the `result` poll are sized to fit one 600 s cell; F0a keeps the 540 s default and its comment; `test_rlm.py::result_uses_wait_and_keeps_its_errors` asserts the default |
| D182 / 0.224.0 | the artifact and closure gates are gone: `find_checker`, `artifact_candidates`, `gate_redrive`, `track_turn`, `Options.cwd`, `Options.gates` deleted; the `gates` config key is migrated away by `ConfigMigration::RemovedGates` (`crates/types/src/config.rs:52-55`) | §6.3, F0c, F0b | no checker discovery exists any more, so the verifier (`plan/verify.rs`) owns it and the cwd; `goal/mod.rs:46` is still `run_check(check, timeout_ms)` with no cwd, so `run_check_in(cwd, …)` is still F0c's to add; F0b's `plans.dir` and `.gitignore` moves follow the `ConfigMigration` precedent rather than inventing a second one |
| D185 / 0.227.0, reverted by D187 / 0.235.0 | D185 deleted orchestrate.md, `Effect::DetachFragment` and doctrine's Planning, Working model, Context and Look before you write sections; D187 restored them byte for byte | §1, F0a, F0c prompt edits | no net change: doctrine rule 4 is at :343-346 and rule 5 at :347-357, orchestrate.md is back; F0a's +20 byte rule-5 edit and F0c's rule-4 edit apply as written. The plan records that these rules were deleted and restored once, so a later cut of them is not a surprise |
| D188 / 0.236.0 | `yi_runtime::session_tools` is the one tool construction; `check_request_budget.py` locks a sha per tool in `scripts/guardrails/baselines/tool_surface.json` (`tool:plan` :24); `just ratchet --update` rewrites it; a moved key owes a Claims ledger section, an added key also a Neighbour matrix and a Dogfood section (`check_pr_metadata.py:196-201`); the request budget is system 28,331, tools 19,383, total 47,714 | §11 conventions (the 45,068 figure at the "No prompt bytes for documentation" bullet), F0a, F0b, F0c, F1d, F4a | every stage that edits a tool description or adds a kernel extra runs `just ratchet --update` for `tool_surface.json` in the ratchet commit and writes the owed PR sections; §11's budget line reads 47,714 (system 28,331, tools 19,383); `plan.op` is an added key, so F0a owes all three sections |
| 0.241.0 and 0.243.0 | the todo tool gained `unleak` (`crates/runtime/src/todo/tool.rs:187`) and accepts the argument shapes the v4 sweep's models sent; the plan parser is `parse_op` at `crates/runtime/src/plan/tool.rs:274-342` behind `request` :344-350 | F0a Files, F0c test table | `plan.op` shares `parse_op` :274-342 exactly as the tool does, so both surfaces accept the same shapes; F0c's `an_unknown_argument_key_is_refused` is written against `parse_op` and must leave the todo tool's `unleak` alone (different file, different tool) |
| 0.244.0 | the prompt-side plan skill `skills/yi/plan/SKILL.md:78` collects with `timeout=420`, and `prompts::no_prompt_example_waits_past_the_cell_ceiling` (`crates/runtime/tests/prompts.rs:155-156`) reads that file; the kernel-side `python/skills/plan/` is untouched since 2d1d7aae (empty diff) and still calls the four unregistered names and claims done is host-verified (`python/skills/plan/SKILL.md:5`) | §1 gap 3, F0a, §12 | the history row's worry that deleting the skill drops a 0.244.0 fix is wrong: the fix landed in `skills/yi/plan/`, a different tree, which F0a leaves alone; F0a deletes `python/skills/plan/` and its `PYTHON_SKILLS` entry only, and the ceiling test is unaffected |
| 0.233.0 (D169) | a `memory` skill joined `PYTHON_SKILLS`, now `[(&str, &str); 5]` at `crates/kernel/src/bootstrap.rs:270-276` | F0a Files, §12 deletions table | the array becomes `[…; 4]` and the entry removed is :274 (`("plan", "plan")`); `("memory", "memory")` :275 stays. The tree, not the history row, fixes the line |

Stage impacts

- F0a. Files line: `plan/tool.rs:344` becomes `crates/runtime/src/plan/tool.rs:274-342` (`parse_op`, shared) with `request` :344-350 fixing the actor and `engine.apply` :527; `wiring.rs:603` becomes :620 (also the rollback line), and the child-link precedent `wiring.rs:508` becomes :524 (`register_child_messaging`, overwriting `subagent.rs:899` with `mailbox.rs:102`); `mailbox.rs:286-319` becomes :286-318 (`take_pending` :304-318, the `record.pending = 0` steal at :312); `python/yi_runtime/src/rlm/__init__.py:52-83,390` becomes :52-85,392 with the 540 s default and D176 comment at :56-58 kept and the ponytail note at :83 removed; `crates/kernel/src/bootstrap.rs:274` stays :274 but the array is `[(&str, &str); 5]` at :270 and becomes `[…; 4]`; `crates/runtime/tests/ext_e2e.rs:350` becomes :340-341 (roster) and :349 (prefix check). Signatures: the handler returns `HostReply` (`crates/kernel/src/client.rs:65`); the `plan.op` reply follows D184's shape (header, changed rows, `next:`), never the whole plan. Tests: `plan_dispatch::a_childs_finish_wakes_an_idle_owner` is an inline test in `crates/runtime/src/plan/dispatch.rs` beside `the_follow_up_wakes_an_idle_owner_and_names_the_held_count` :525, on `crate::scratch::Scratch`; the existing-test row cites `ext_e2e.rs:364`. Prompt bytes: rule 5 at `doctrine.md:347-357` is exact; the row also states the `tool_surface.json` re-lock (`plan.op` is an added key: Claims ledger, Neighbour matrix, Dogfood) and the 47,714 budget. The skill deletion is `python/skills/plan/` only; `skills/yi/plan/SKILL.md` stays.
- F0b. Files line: `wiring.rs:456` becomes :469-473; `crates/cli/src/rpc.rs:217-230` exact. §5 cites: lease `store.rs:321-364` (staleness via `yi_kernel::bootstrap::lock_is_stale` :342, `create_dir` :325, hold :162-168, `Drop` :188-191), `split_frontmatter` :76, `parse_document` :110, `DocumentError` :67, `user_edits` :366-386, write path :254-270, `PlanId::child` `ops.rs:925`, kinship :979-982, `Op` :46-102, `OpKind` `table.rs:13-29`, `op_name` :139-157, `config.rs:167`, `schema.rs:38-39`. The reducer's `HostReply`-typed replies reuse `client.rs:65`. Prompt and doc changes: the "-60 bytes from the file sentence" line is deleted (no such sentence at `tool.rs:569`); if the DESCRIPTION changes at all, the PR runs `just ratchet --update` for `tool_surface.json:24` and owes a Claims ledger. The `plans.dir` relocation follows `ConfigMigration` (`config.rs:52-55`). File placement: `ops.rs` has 104 lines of headroom, so `journal.rs`, `state.rs`, `recovery.rs`, `import.rs`, `artifact.rs` are required, not optional.
- F0c. Files line: `validate_product` is already a free function at `ops.rs:288-313`, called from the done block :810-844; the implicit pass the `unserved-output-today-passes.json` fixture pins is :838-840; `check_terminal` :845. `goal/mod.rs:46` is exact, and after D182 nothing else discovers a checker or a cwd, so `Verifier` owns both; `Verifier.timeout_ms` reads the session `Deadline` (D177) as well as its own clock. `tool.rs:344-350` in the test table becomes `parse_op` :274-342. `table.rs:163` grants `Child` only `View`; `Submit` is a new `OpKind` this stage adds. `checkpoint.rs:16-27` becomes `crates/tools/src/checkpoint.rs:40-46`, `capture` :75. Wall cites: `wall.rs:13` exact, prefix rule :34-36, `check_read_path` :97-106, cooperative doc :108-109, enforcement at `fetch/mod.rs:388` and `fetch/schemes.rs:128,290` rather than `tools.rs`. `plan/dispatch.rs:84-107`, `doctrine.md:343-346`, `crates/types/tests/wire_roundtrip.rs:136,209,222-233` and `extract.py:389-397` are exact. The refusal text obeys D183: no typography rule on evidence. The contract on `Todo` changes the plan tool's schema, so this PR also re-locks `tool_surface.json` and owes the D188 sections.
- F0d. Files line: `lane/mod.rs:939-955,1013-1052` becomes :941-957 (`merge_into`, incident note :941) and :1013-1055 (`take_settled_worktree` :1035-1053, Running refusal :1044-1048); the file is 1,055 lines, so `plan/acceptance.rs` stands; `subagent.rs:851-858` becomes :852-859; `mailbox.rs:491` is exact but `reap` now returns `Harvest` (:491-545, struct :546) and promotes the last product before freeing the slot, so the disposition is a field on `Harvest`, not a replacement for a drop; `worktree_state` and `settle` in `plan/dispatch.rs` are new symbols; `probe.rs:105,233-260`, `family.rs:136-161` and `StuckLatch` (new) are as stated. `Lane::settle` reads the session `Deadline` (D177) so an integration check cannot outlive the run.

## 3. Decisions

### 3.1 Standing rules the plan obeys

- **Mechanism in the kernel, policy in user space, data in the triggers.**
  Nothing in loop control (stop posture, nudges, ladders, admission,
  transition legality, verdict aggregation, abandon, stuck) fires on a
  model's judgment. LLM judgment lives only inside gated envelopes:
  enumerated outcomes, a fail-closed default, a recorded verdict, a counter
  bounding repeats. The judge tier (§6.4), review pods (§11 F3), permission
  auto-review (`crates/runtime/src/auto_review.rs`, the template), and the
  plan program itself (§8) are such envelopes.
- **No new crates** (`scripts/guardrails/baselines/deps_budget.json`: 20
  direct, 167 transitive). **No new `YI_*` env var** beyond `YI_LEVERS`, read
  only in eval mode (§10), declared in `baselines/env_vars.json` (7 of 40).
  **No new `AgentEvent` variant**: every new syscall is a host request on
  the registry (`kernel.rs:154`), never an event, so the design-time cap
  (`docs/YI_DESIGN.md:1260,1282-1284`) holds at fourteen.
- **No prompt bytes for documentation.** The Python surface documents
  itself through `help(yi)` in the kernel; the request-prefix budget
  (`baselines/request_budget.json`: system 28,320, tools 16,748, total
  45,068) moves only by `--update` in its own commit named in the row
  (D137); `check_prompt_examples.py` stays green and its name set widens
  from `rlm.` to `plan.`, `shapes.`, `yi.` (§8.7).
- **Guardrails**: files ≤ 1,200 lines, functions ≤ 150; ratchets shrink only
  with a `growth +N:` memo in the version's row past +150 and a D cite past
  +2,000; one changelog row and one ADR per PR; D numbers claimed at land
  time; fixtures before source; every kernel invariant gets a test that dies
  with it (§11 tables); never relax a linter; stacked PRs land in order; the
  forge holds the issues, one per stage under one milestone, opened before
  the row.
- **The plan never requires the kernel.** The JSON plan tool stays a complete
  degraded surface over the same engine; the typed store stays the durable
  path; the program is the rich path. The kernel was dead in every early paid
  trial (D160, `docs/ARCHITECTURE.md:154`). Every stage in §11 names its
  kernel-dead path.

### 3.2 The store decision (made; amends D97 and D105, carries D26 and D53)

`.yi/plans/<slug>/plan.json` (typed snapshot, format 2, sorted keys, pretty
printed, validated on read against a JSON Schema published to
`.yi/schemas/plan.schema.json`), `.yi/plans/<slug>/ops.jsonl` (append-only
`PlanOpRecord` log, a second `OpSink` beside the session sink), and
`.yi/plans/<slug>/program.py` (source, when a program wrote the plan). No
Markdown anywhere in the store; Markdown is a computed view (`yi plan view
--md`) and an input syntax of the degraded tool's `set` op (`ops.rs:94`),
never a stored document. No hand edits: `fold_user_edits` and
`store.user_edits` are deleted; the user changes a plan by prompting Yi or
through `yi plan <op>` as `Actor::User` carrying a `user://` citation,
widened from Unblock and View to every op; `yi plan fuse reset` and `yi plan
repair` are two more cited user ops. D26 ("no advisory-only state tool") and
D53's anti-laundering intent (expand-only, supersede audited) carry forward
unchanged. §5 has the shapes.

### 3.3 The mechanism and policy split, made concrete

| concern | today | target home | why it moves |
|---|---|---|---|
| transition legality | `table.rs:38-141` | kernel, unchanged | the one thing only the kernel can promise |
| admission (width, 8 per parent, 16 per family, fuse 64) | `table.rs:227-237`, `subagent.rs:18,20`, `ids.rs:340` | kernel, as refusal only | over-admission is refused with the count; nothing is silently held |
| scheduling order | `Plan::ready()` Vec order (`doc.rs:583`), `admissible` takes them in that order | user space: the shape decides which ready todo starts next | order is policy; the exokernel keeps protection and gives up management |
| retry policy (when, with what) | the owner's ladder retry → decompose → supersede, by prompt | user space: a shape's restart strategy; the kernel keeps `RETRY_CAP` 8 as a fuse | intensity is policy; the cap is a fuse |
| model choice | `SpawnSpec.model` written by the model, `kwargs_of` passes it | user space: roles and programs; the kernel passes it through | choice is content |
| verification of done | nothing (`ops.rs:803-839`) | kernel verifier over contract items (§6) | a promise the owner cannot fake |
| revocation | none (delete or interrupt, no record) | kernel leases: revoke(grace), abort, repossession record (§7) | visible revocation is the exokernel's second law |
| capabilities | `rlm.run` kwargs only (`subagent.rs:273-282`); plan-dispatched children get none | kernel: `Wall` on `SpawnSpec`, shrinks hereditarily, holds compile down (hive L1, L4) | authority only shrinks |
| documentation of the API | prompt bytes | `help(yi)` in the kernel | paid once at import, not per turn |

### 3.4 Cut, and not planned

Dry runs; a shared-tree lint; a "verification economy"; contract-net bidding;
pub/sub topics; any exactly-once promise (delivery is at-most-once with a
durable inbox and receipts, §7); cross-root messaging; the hive; A2A
adapters; module regeneration; cell replay as recovery (§0 S5); an OLS or
Gaussian-process sweep over the levers manifest (§0 R12); `Abandon` as a
parent-close policy until a supervisor owns orphans (§7.4). Seams reserved
and nothing more: `placement`
on `Delegation` (null; `docs/plans/2026-09-01-hive.md:350-352`),
`Blocked { on: Partition }` as a `BlockedOn::Other("partition")` spelling the
table already accepts (`doc.rs:78-79`), and the capsule manifest format
(hive §7.4, :320-331) as the shape a future `capsule freeze` writes.

### 3.5 ADR and D-row amendments

| D-next | title | amends / carries | stage |
|---|---|---|---|
| D-next-1 | plan ops are one host request, `plan.op`, over the one engine; a child's lifecycle notice wakes its parent through the idle-waiting hook; `wait` is cursored and returns states | extends D137's split (todos are a session tool, plans a ledger) and D165 | F0a |
| D-next-2 | the plan store is an append-only journal with a typed JSON checkpoint and artifact blobs; recovery reconstructs without effects; no Markdown documents, no hand edits; user authority is a confirmed channel | amends D97 and D105; carries D26, D53 | F0b |
| D-next-3 | completion is verified by the kernel on every path against a frozen attempt; a critical abstention cannot pass; `set` cannot author Done | retires D77's ladder fields (nothing reads them); carries D77's intent (deterministic protocol) | F0c |
| D-next-4 | a worktree todo is done only when its candidate and its integration pass and the acceptance is recorded; every other exit records a disposition | new | F0d |
| D-next-5 | plans are programs in the kernel: the `yi` library, handles, idempotent todos, explicit resume, source recorded and never replayed | extends D166 (the working model) | F1a |
| D-next-6 | two shapes are library schedulers under the engine's admission and step table: fork_join and scatter, with quotes verified against the archive | extends D166 | F1b |
| D-next-7 | paged recall through `fetch` | extends D164 | F1c |
| D-next-8 | messages are envelopes with kinds, receipts, per-pair order and a durable inbox | extends D165 | F2a |
| D-next-9 | a child holds a lease drawn from its parent; revoke has a grace and a repossession record | new | F2b |
| D-next-10 (D216) | a judged contract item is a walled reader of another model family answering a fixed schema, aggregated in Rust | new | F3a |
| D-next-11 (D217) | a review pod is readers plus a code arbiter | extends D-next-10 | F3b |
| D-next-12 (D218) | a service is a child with a stable address | extends D165 | F3c |
| D-next-13 (D219) | the procedural graph replaces hand-maintained affordance strings; frozen online, evolved offline under a held-out gate with rejection memory | amends the affordance rows (D139's ladder stays) | F4a/F4b |
| D-next-14 | the kernel's constants are levers with floors and two gates; the sweep never reads the held-out split | extends D140 | F4c/F4d |

### 3.6 The completion invariant and the authority rule

A managed todo is `Done` only when all of the following hold, whatever
surface asked: the verification token names the current attempt; the frozen
contract and its criteria match; every critical item passed; score and
coverage meet the frozen policy; the accepted output snapshot is the verified
one; any required integration is recorded and verified; the verdict and the
transition are one committed journal record. `done`, `set`, `import`,
`repair`, `supersede`, the CLI and every shape go through this one validator;
`set` may declare and rearrange work and request legal transitions, never
author a resolution (F0c landed the one validator, D194; a todo with no
contract and no stated acceptance still completes on the caller's word as
`Done { resolution: None }`, reported unverified, until a decider or F3a's
judge gives it one: the carve-out D194 records). Imported legacy success is
`LegacyUnverified`, displayed as history, never as new evidence; a user's
administrative acceptance is `AcceptedByUser`, distinct from `VerifiedDone`
in the store and in every report. `Plan::finished()` (`doc.rs:598`) counts
Failed and Abandoned; it is a lifecycle predicate, not a success predicate,
and reports say which.

Authority is a channel, never a string. The host derives the principal from
the registry, the tool or the console a request arrived on; request JSON
carries no actor; a child may view its parent's plan and submit results for
its own attempt only. Administrative ops (`fuse reset`, `repair` resolutions,
legacy acceptance) are confirmed through the session's existing permission
path (`Decision::Ask`, `crates/runtime/src/gate.rs:112-126`), bound to root,
op, argument hash and expected revision, consumed once; a CLI invoked by an
agent keeps the agent's principal. `user://` is a provenance citation, not a
proof. Walls stay cooperative (`wall.rs:114-115`: "Not a sandbox"); every
sentence below that says "walled" means that, and nothing claims process or
filesystem isolation this tree does not have.

### 3.7 High-assurance review (the `har` skills, 2026-09-13)

The plan was read against the twelve `har-*` skills (`~/Development/har-skills`);
the ones that bind here are `har`, `har-io`, `har-async`, `har-threat`,
`har-verify`, `har-concurrent`, `har-api`, `har-layout`, `har-supply`. What
the tree already enforces: the panic budget is a workspace lint
(`Cargo.toml:76-84`: `unwrap_used`, `expect_used`, `panic`, `todo`,
`unimplemented`, `await_holding_lock` all `deny`); a checker subprocess is
already its own process group and is killed as a tree
(`crates/tools/src/process.rs:31,118-124`); temp names come from a
pid-plus-counter nonce (`store.rs:19-26`), which is fine for a file name and
is never a security token. What the review changed, by skill:

| skill | rule | where the plan now says it |
|---|---|---|
| `har` | newtype every id and unit scalar; validate at construction; checked arithmetic; no `_ =>` on owned enums | §6.1: `AttemptId`, `RequestId`, `EffectId`, `Seq`, `Digest`, `Weight (1..=100)`, `Permille (1..=1000)` with checked constructors; `aggregate` divides by a `NonZeroU64` and uses `checked_mul`; `attempt` and `refusals` advance by `checked_add` and refuse at their caps |
| `har` | `Result` on every fallible path; never `let _ =` an error that matters | §5.3: `OpSink::record` returns `Result` for the journal and stays infallible-by-policy only for telemetry; the write-failure reap at `ops.rs:537` (`let _ = self.delegate.reap`) becomes a recorded reconciliation, not a dropped error |
| `har`, `har-api` | typestate for a small mandatory order; `#[must_use]` on evidence a discard would lose | §6.6: `Candidate<Submitted> → Candidate<Verified> → Integration<Verified> → Accepted` as typestate (the marker is `PhantomData<fn() -> S>`); `#[must_use]` on `Lease`, `Receipt`, `VerificationToken`, `Verdict`, `Disposition`; `Lane::settle(self) -> Result` is the fallible release and `Drop for Lane` stays best-effort |
| `har-io` | name the commit point; `sync_all`; tmp-plus-rename in one filesystem; sync the directory; `create_new`; one `write` is not one record | §5.3: the journal append is `write_all` of one newline-terminated record, then `sync_data` on the journal file (the commit point), then the checkpoint by `File::create_new` on a temp file in the same directory, `sync_all`, rename, directory `sync_all`; the lease's `create_dir` is the atomic primitive it already uses (`store.rs:324`) |
| `har-io` | model the outcome, not the call: `NotStarted / Committed / Failed / Unknown`; never blind-retry `Unknown` | §5.3 and §5.6: every effect intent reconciles to that enum; `Unknown` is `NeedsReconciliation` and is never re-issued automatically |
| `har-io` | `Instant` for deadlines local to a process, `SystemTime` for records; the clock is injectable | §6.3 and §7.4: the verification deadline and the revoke grace are `Instant`s computed once at entry and passed down; journal `at` fields are wall-clock milliseconds; a child's inherited deadline crosses processes as wall-clock milliseconds under the stated one-host, one-clock assumption and is turned into a local `Instant` at receipt; `probe.rs`, `lease.rs` and the verifier take a `Clock` trait, and the tests drive rollback, skew and the exact-boundary expiry |
| `har-io` | never slice a `str` by an offset you did not get from it; a limit states its unit | §5.5: a body section over 4 KiB goes whole to an artifact, never cut; every cap in this plan is bytes, and says so |
| `har-async` | a `spawn_blocking` job cannot be aborted; bound it by its own means; `abort()` is not a barrier; shutdown is signal, wait with a deadline, force, join | §6.3: the checker runs under the existing `CancelFlag` (`goal/mod.rs:52`) tied to the verification `Instant`, in its own process group with `kill_tree` (`process.rs:31`), output capped; §7.4: revoke is cancel envelope → grace → `interrupt` → kill the group → **join** → settle → commit; nothing is reported terminated before the join returns; the verifier's tasks live in a `JoinSet` the host shuts down, never a detached handle |
| `har-async` | a future dropped mid-flight is cancellation | §6.3: the `plan.op{done}` request may be dropped by the kernel comm at any await; the verification effect is committed at step 2, so a dropped request leaves an intent without a result, which recovery reconciles; the effect never depends on the request's future surviving |
| `har-async` | `Notify` stores one permit; `notify_one` before `notified()` is not lost | §7.4 and §7.5: the probe loop's wake is a `Notify`, chosen for exactly that property |
| `har-concurrent` | decide the poison policy per lock; scope guards; no await under a guard | new modules follow the tree's policy (`unwrap_or_else(PoisonError::into_inner)`, `fetch/log.rs:36`); cursors and pair sequences live under one host mutex, never atomics; the `await_holding_lock` lint stays |
| `har-threat` | draw the boundary, walk STRIDE, test the control | the table below; F0b's PR adds a "Trust boundaries" section to `docs/ARCHITECTURE.md` (the file `har-layout` names as its home) |
| `har-threat` | secrets never in argv or a child's environment; launch by absolute path; minimal environment; bounded reads and waits | §6.3: the verifier launches `/bin/sh` by absolute path with `env_clear()` and an allowlist (`PATH`, `HOME`, `LANG`, `TMPDIR`, the contract's declared variables); today `run_captured` inherits the caller's environment (`process.rs:110-127` sets no `env_clear`), so a checker running repository code would see provider keys: the test `verify::the_checker_sees_no_provider_key` dies with the control; every checker has a wall-clock bound, an output cap and a process group |
| `har-threat` | the agent boundary: provenance travels, text becomes action only through a typed allowlist, authorization is decided outside the model, confirmation for destructive effects, persisted memory is an injection channel | envelopes keep the `<agent_message from=…>` wrapper (`mailbox.rs:59-61`); `plan.op` is the typed allowlist; `check_actor` is the policy; `fuse reset`, `accept` and resolutions are confirmed; `plan.json` and `ops.jsonl` are re-read state and are digest-checked and schema-checked on every read; the judge's rule that evidence addressing it is data (`auto_review.md:5-9`) is reused verbatim |
| `har-threat` | per-format budgets: a byte-limited reader in front of every decoder; depth bounds; no `Vec::with_capacity` from a peer length | §5.3: the journal reader takes `Read::take(record_cap)` per line and resynchronizes at the next newline on an over-long record (a torn tail is the only tolerated damage); `serde_json`'s default recursion limit stays (no `unbounded_depth`); `plan.json` has the 32 KiB cap before parse; a `plan.op` payload is bounded by the kernel comm's frame cap and its `args` by the same 32 KiB |
| `har-threat` | canonicalize-then-check is TOCTOU | §7.6 says so about `Wall::check_read_path` (`wall.rs:100-110`): cooperative, not a sandbox; artifact blobs are written with `create_new` under the plan directory and read by digest |
| `har-verify` | climb the ladder to the risk and record what each rung proved | the ladder per stage is in §11's conventions: rung 3 (`proptest`) already drives the engine (`plan_fuzz.rs`) and gains journal crash points and the `aggregate` never-panics and monotonicity properties; rung 3.5 (`cargo mutants`) runs on `contract.rs`, `journal.rs`, `state.rs`, `recovery.rs` in a nightly job and every surviving mutant is triaged in the PR; a real kill-9 crash test (T1) supplements the injected-failure tests; rung 8 (`kani`) is offered for `aggregate` (16 items, bounded) in its own pinned lane, not required; coverage is a diagnostic, never a gate |
| `har-api` | derive the house line on ids; `#[non_exhaustive]` on public error enums; `From` not `Into`; document `# Errors` | §6.1 types carry `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]` on ids; the crates are workspace-internal (`Cargo.toml:22`, nothing published), so `#[non_exhaustive]` is used where the tree already uses it and not retrofitted; every new `pub fn` gets `# Errors` |
| `har-layout` | modules named by what they own; no `util`; serde `try_from` newtypes; no `#[serde(default)]` on required fields | the new files are `journal`, `state`, `recovery`, `import`, `artifact`, `verify`, `acceptance`, `lease`, `mail`; `format`, `seq`, `request_id` carry no serde default; validated newtypes use `#[serde(try_from = "String")]` as `Url` does (`url.rs:159`) |
| `har-supply` | no new dependency without the gate; `overflow-checks` in the shipped profile; workspace `forbid(unsafe_code)` | F0-F2 add no dependency (`sha2`, `proptest`, `tokio::sync::Notify` are present); the process-group kill is std's `process_group(0)` plus the tree kill the tools crate already has, so no `libc::killpg`; two items to verify at F0b: whether the shipped `dist` profile sets `overflow-checks = true` (no `[profile.release]` block was found in `Cargo.toml`), and the crate-root `unsafe` posture (`[workspace.lints.rust]` carries no `unsafe_code` line at `Cargo.toml:70-84`) |

Trust boundaries this plan adds or touches, one STRIDE instance each, and
the test that dies with the control (the full walk is the F0b ADR's
appendix):

| boundary | what crosses | assume | instance | control | test |
|---|---|---|---|---|---|
| child kernel → `plan.op` | JSON args from a model | hostile, attacker-chosen | E: a child mutates its parent's plan | actor fixed by the registry, `Child` gets `View` and `Submit` only (`table.rs:163`) | `plan_e2e::a_child_kernels_plan_op_is_refused_beyond_view` |
| console rpc → administrative op | a request to reset the fuse or accept work | the socket peer is anything on the box | S: a local process claims the user | `Actor::User` is minted only by the process that owns the human's permission prompt; a socket peer can submit, never confirm (verify the daemon socket's auth at F0b; if it is unauthenticated, confirmation stays inside the TUI or console process) | `plan_ops::agent_cli_cannot_reset_fuse_as_user`; `rpc::a_socket_client_cannot_confirm_an_administrative_op` |
| host → checker subprocess → host | exit code, stdout of repository code | hostile: the repo's tests are attacker-influenced | I: provider keys in the checker's environment | `env_clear` plus allowlist, absolute `/bin/sh`, process group, output cap, deadline | `verify::the_checker_sees_no_provider_key`; `verify::a_checkers_grandchild_is_killed_at_the_deadline` |
| judge child → verifier | a model's verdict text | hostile; evidence carries instructions | T: a forged `pass` or a quote never fetched | fixed schema, fail-closed parse, quotes checked against the judge's own fetch log, quorum | `judge::malformed_or_empty_answers_abstain`; `judge::an_unbacked_quote_abstains_the_item` |
| disk → engine (`plan.json`, `ops.jsonl`, artifacts) | files another process may edit | stored data is as trusted as its writer | T: a schema-valid edited checkpoint | digest chain to the journal, schema and domain validation, `ExternalEditDetected`, blobs by digest | `plan_store::edited_export_cannot_overwrite_authoritative_state`; `plan_recovery::corrupt_middle_record_blocks_recovery` |
| child → parent mailbox | envelopes | hostile text | D: a progress flood starves a cancel | bounded bodies, per-sender request caps, reserved control capacity, coalesced progress | `mailbox::a_body_over_sixteen_kib_is_refused_not_trimmed`; F2a's flood test |
| format-1 file → importer | a Markdown document | user-authored, possibly huge | D: a 4 GiB body | the 32 KiB frontmatter cap before parse, sections to artifacts by digest, bounded reader | `plan_import::import_preserves_all_markdown_and_large_notes` |
| `family://`, `tree://`, `history://` → a reader model | a family member's files and transcripts | data, never instructions | R: a quote with no fetch behind it | the fetch log backs every citation (`fetch/log.rs:82`) | `test_yi_shapes::scatter_drops_an_unverifiable_quote_before_the_lead_sees_it` |

Letters with no plausible instance are written down, not skipped: repudiation
is closed everywhere by the journal (every accepted request writes its record
before the effect runs, §5.3), and information disclosure on the disk boundary
is the user's own filesystem permissions, which this plan does not change.

## 4. Architecture

### 4.1 The diagram

```
human -- prompt, console, `yi plan <op>` (Actor::User, user:// cited)
  |
USER SPACE, one IPython kernel per process (crates/kernel; python/yi_runtime)
  shell cells; recipes (templates) -> plan programs (disposable instances);
  shapes as library schedulers (pipeline, fork_join, map_reduce, tournament, pod, scatter);
  readers and writers as library roles; scheduling order lives here; help(yi)
  |  syscalls: the host registry (crates/runtime/src/kernel.rs:154)
KERNEL, yi-runtime
  process table + step table | admission as refusal only (width, 8/parent, 16/family, fuse 64)
  verifier: cmd, schema, example, judge envelope | mailbox: envelope, kinds, wake
  leases: revoke(grace), abort, repossession record | capabilities: walls, shrink hereditarily,
  holds compile down | lanes | VFS with paged recall | timers | ledger
  |
STORAGE: .yi/plans/<slug>/{plan.json, ops.jsonl, program.py}; session store; lane pool; git
PROCEDURAL GRAPH: versioned, typed, frozen online, evolved offline (crates/runtime/src/prompts/graph.json)
DEVICES: models via providers (crates/ai); tools (crates/tools); MCP (crates/mcp-cli)
init: yi serve, session registry, one worker per root
family: root -> child (own kernel, tree, lease) -> grandchild, depth <= 3; inline todo = thread
```

Delta against the prior draft: revocation added to the kernel; scheduling
order moved to user space; the procedural graph added beside storage;
placement reserved on `Delegation`.

### 4.2 Primitives glossary

| primitive | today (code home) | target home |
|---|---|---|
| process | a child session: `SubagentHost::spawn` (`subagent.rs:470`), `ChildId` (`crates/types/src/subagent.rs:4`), its own kernel, tree, transcript | same, plus an address, a lease and a wall (§7) |
| thread | an inline todo `Running { by: main }` (`ops.rs:774-775`); a `bash()` job handle (`kernel.rs:165-238`) | same; a kernel todo is a coroutine in the owner's kernel (§8.4) |
| kernel todo | none | `plan.todo(label, run=coroutine)`: work in the owner's kernel under the plan's step table, no admission slot (`table.rs:231-232` already exempts undelegated todos) |
| plan | `Plan` (`doc.rs:457-469`), `PlanFile { plan, body }` (`store.rs:138-141`), one `.md` per id | `plan.json` + `ops.jsonl` + `program.py` under `.yi/plans/<slug>/` (§5) |
| todo | `Todo` (`doc.rs:210`), `TodoState` (`doc.rs:85`), `TodoRepr` (`doc.rs:315-340`) | same, plus `contract`, `contractHash`, `refusals`, `note` (§5.2) |
| contract | `Delegation.accept: Check` (`doc.rs:60-65,196`), unread at done | `Contract { items, threshold, class }` in `yi-types`, run by the kernel at done (§6) |
| edge | `after: Vec<TodoLabel>` (`doc.rs`), `clears_edge` (`doc.rs:119-121`) | unchanged |
| shape | prose patterns in `orchestrate.md` (D166) | `yi.shapes` library schedulers with restart strategies (§8.5) |
| message | `mailbox.rs` route/deliver (`agent_message.send`, `mailbox.rs:102-112`), a child's report as its last text | an `Envelope` in `AgentMessage::Custom.details` with kinds, receipts, per-pair order, a durable inbox (§7) |
| address | a child's name (`ChildId`), `agent://<plan>/<todo>` (`TodoAddr`, `ids.rs`), `plan://`, `history://`, `user://` (`url.rs:7-17`) | unchanged; a service is a child whose name is stable across respawns (F3c) |
| lease | none; `wiring.deadline` is a session-wide clock (`environment.rs:141`); `SpawnSpec.budget` (`doc.rs:177`) is carried, not enforced | `Lease { holder, parent, deadline, tokens }` drawn from the parent, returned at reap, revocable with grace (§7.4) |
| capability | `Wall` from `rlm.run` kwargs only (`subagent.rs:273-282`; `wall.rs`) | `SpawnSpec.wall`, hereditary shrink, holds compiled down at spawn (§7.6) |
| timer | the probe ladder (`probe.rs:14-24,105`); the heartbeat scheduler (`schedule/mod.rs`) | same two, carrying the stuck tick (F0d) and revoke grace (F2b) |
| supervisor | the owner's ladder by prompt (retry → decompose → supersede) | a shape's restart strategy (`one_for_one`, `rest_for_one`, `one_for_all`; intensity `max` within `window`), under the kernel's `RETRY_CAP` fuse |
| ledger | `custom{plan_op}` on the session (`plan/ledger.rs:9-25`), `custom{fetch}` (`fetch/log.rs:39`), `yi plan report` (`ledger.rs:173`), `yi why` (`why.rs`) | plus `ops.jsonl` per plan, `done_refused`, `repossession`, `verdict` records; `evals/levers.py` reads them |
| procedural graph | `next:` lines as hand-maintained strings (`crates/runtime/src/affordance.rs`; `todo/text.rs:6` `NEXT_LINES = 3`) | `prompts/graph.json`, typed nodes and edges over Yi's verbs, rendered by `affordance.rs` (§9) |

### 4.3 The syscall table

| exists (registered today) | new | removed |
|---|---|---|
| `rlm.run`, `rlm.wait`, `rlm.result`, `rlm.status`, `rlm.interrupt`, `rlm.delete_subagent`, `rlm.list_subagents`, `rlm.find_models`, `rlm.merge_worktree`, `rlm.discard_worktree` (`subagent.rs:899-1027`) | `plan.op` (F0a): one request, `{request_id, plan?, expected_revision?, op, args}`, actor fixed by the registry (Owner for the root kernel, Child(name) for a child kernel); the reply carries the typed result or refusal, the current revision and the durable request id | the four names the plan skill calls and nothing registers (`plan.create`, `plan.update`, `plan.edit`, `plan.split`): the skill is deleted in the PR that ships `plan.op` (F0a) |
| `agent_message.send`, `agent_message.list_agents` (`mailbox.rs:102-115`) | `rlm.wait` takes a per-caller `cursor` and returns `{cursor, changed, states, notes}` (F0a; same name; the old `updated` field kept one release) | |
| `plan.get` (`plan/mod.rs:379`) | `plan.op{op: program}` appends a cell to `program.py` (F1a; an op, not a request) | |
| `goal.get`, `goal.create`, `goal.update` (`goal/mod.rs:562-583`) | `fetch` takes `offset` and `limit` (F1c; same name) | |
| `fetch` (`wiring.rs:209`), `history.grep` (:256) | `agent_message.send` takes `kind`, `conversation`, `reply_to`, `deadline_ms` (F2a; same name); `agent_message.request` sends and waits for the reply (F2a; Python `rlm.request`) | |
| `compact.run`, `compact.status` (`wiring.rs:471-485`) | `rlm.revoke` (F2b) | |
| `rlm_heartbeat.list/create/update/delete` (`schedule/mod.rs:1010-1121`) | `rlm.service` (F3c: spawn with a stable address; a respawn keeps the name) | |
| `exec.spawn/tail/poll/kill/release` (`kernel.rs:165-238`) | | |
| `model.info` (`subagent.rs:1027`) | | |

Every "new" entry is a host request or a widened reply on an existing one;
no `AgentEvent` variant is added (§3.1).

### 4.4 The three surfaces and the computed views

| surface | who | path | degraded when |
|---|---|---|---|
| Python, rich | the model in its kernel | `import yi` → `plan.op` and the `rlm.*` requests | the kernel is dead: the model has the JSON tool |
| JSON tool, degraded | the model without a kernel | `plan` tool (`tool.rs:523-527`) → the same `PlanEngine::apply` | never; it is the floor |
| CLI and console, user ops | the human | `yi plan <op> [--json args]` (`crates/cli/src/plan.rs`) as `Actor::Owner`; administrative verbs are submitted to the running session and confirmed through its permission path, which mints `Actor::User` (§3.6, §5.6); console rpc `plan` gains those actions (`rpc.rs:217-230`) | never |

Computed views (all read the same `plan.json` and `ops.jsonl`): `plantree`
in the TUI is `ps` (`crates/tui/src/plantree.rs`, `crates/tui/tests/plantree.rs`);
`yi plan report` is `top` (`ledger.rs:173`: wall, critical path, per-todo
running/waiting/blocked, at-init vs widest); `yi why <file>:<line>` is
`blame` (`why.rs:91`); `yi plan view --md` renders the Markdown the store no
longer keeps.

## 5. The store

### 5.1 Directory layout

```
.yi/plans/
  .lease/                       the store lease, unchanged (store.rs:11-15, 320-348)
  .gitignore                    written once by the store: "*/ops.jsonl"
  <slug>/plan.json              the typed snapshot, format 2
  <slug>/ops.jsonl              the append-only op log (one line per applied or refused op)
  <slug>/program.py             an audit export of recorded cells, when a program wrote the plan (F1a); never executed
  <slug>/artifacts/<sha256>     immutable blobs: frozen criteria, submitted outputs, recorded source, the imported original
  <slug>.<child-slug>/…         a sub-plan keeps its dotted id as its directory name
                                (PlanId::child, ops.rs:927; kinship by prefix, ops.rs:973-976)
.yi/schemas/plan.schema.json    published on store open from yi-types (include_str!), read by programs and by yi plan repair
```

`plans.dir` (`crates/types/src/config.rs:32,165`; `wiring.rs:456`) still
relocates the whole directory. `.yi/plans/` stays git-tracked as D97 says;
`ops.jsonl` is ignored by the store's own `.gitignore` line because an
append-only log in git is churn with no reader (open decision 13.4).

### 5.2 `plan.json`, format 2

Built from `PlanRepr` (`doc.rs:481-499`) and `TodoRepr` (`doc.rs:315-340`);
`PLAN_FORMAT` becomes 2 (`ids.rs:5`); `DocError::Format` (`doc.rs:527-532`)
refuses anything else after import. Sorted keys, two-space pretty print
(`serde_json` with `preserve_order` is already a workspace feature,
`Cargo.toml:44`; sorting is one `BTreeMap` pass at render). New fields are
marked `+`.

```json
{
  "format": 2,
  "plan": "ship-logrotate-lite",
  "goal": "ship logrotate-lite with a packaged tarball",
  "+intent": "one paragraph the program or the owner wrote; ≤ 2 KiB",
  "+constraints": ["no new dependency", "python 3.11+"],
  "+examples": ["local://.yi/plans/ship-logrotate-lite/examples.json"],
  "+shape": "fork_join",
  "+placement": null,
  "version": 1, "touched": 7, "tier": "root", "spawns": 2, "state": "active",
  "todos": [
    {"label": "write the test suite", "state": "running", "by": "tests",
     "after": ["freeze the CLI surface"],
     "+contract": {"class": "writer", "threshold": 1000, "min_coverage": 1000,
                   "items": [{"id": "pytest", "decider": {"cmd": {"checker": "artifact:sha256:…", "timeout_ms": 600000}}, "critical": true, "weight": 1},
                             {"id": "report", "decider": {"schema": {"schema": "artifact:sha256:…"}}, "critical": true, "weight": 1}]},
     "+attempt": 2,
     "delegation": {"spec": {"isolation": "worktree", "+wall": {"deny_write": ["docs/"]}},
                    "output": {"schema": "local://.yi/schemas/test_report.json"},
                    "context": ["plan://ship-logrotate-lite/freeze-the-cli-surface"],
                    "+placement": null},
     "+contract_hash": "sha256:…", "+refusals": 0, "retries": 0,
     "+note": "prose imported from a format-1 body section, ≤ 4 KiB"}
  ]
}
```

Field names are the Rust names verbatim: `PlanRepr` and `TodoRepr` carry no
`rename_all` (`doc.rs:480-481,314-315`), so the snapshot is snake_case;
the op record is camelCase because `PlanOpRecord` is
(`crates/types/src/plan/ledger.rs:11`).

- `accept: Check` (`doc.rs:196`) is replaced by `contract: Contract` on the
  todo itself (§6.1), whether the todo is inline or delegated; `Delegation`
  describes execution and context only. Import maps `Check::Command(c)` to
  one critical `cmd` item marked `revalidate` (no verdict is imported) and
  `Check::Stated(t)` to a `LegacyUnverified` requirement kept verbatim; a
  todo with only a legacy requirement cannot complete until a decidable item
  is added or a user accepts it (§3.6).
- `attempt` counts attempts of the todo; a retry is a new attempt; verdicts
  and tokens name the attempt (§6.3).
- `spawns` keeps its monotonic invariant; the only way down is the confirmed
  user op `fuse reset` (§5.6), which replaces the hand edit named at
  `doc.rs:463-464`; a supersede, a reconciliation or a kernel restart never
  lowers it, and a retry of one committed `spawn_intent` is charged once.
- The 32 KiB cap (`store.rs:9`) keeps its name and value; a contract-rich
  plan of forty todos measures about 24 KiB at 600 bytes per todo, and the
  cap is still refused before the write, never trimmed (four-primitives §5,
  :454-460).
- Schema: `crates/types/src/plan/plan.schema.json`, `include_str!` into
  `yi-types`, the subset `crates/runtime/src/schema.rs` validates
  (`type/required/properties/items/enum`); `PlanStore::read` validates before
  `serde_json::from_str::<Plan>` and refuses with the validator's own path
  (`schema.rs:41`, e.g. `$.todos[3].state: expected string`). The published
  copy under `.yi/schemas/` is written on `PlanStore::open` when absent or
  stale (byte compare).

### 5.3 `ops.jsonl` is the journal; `plan.json` is its checkpoint

One record per transaction: an applied op, a refusal, a verification token,
a spawn intent or result, an import, a reconciliation. The record is the
commit point: appended under the store lease the op already holds
(`ops.rs:399`), newline-terminated, carrying a `digest` (sha256 of the
canonical record chained to the previous one), flushed to stable storage
before the engine touches `plan.json`; then `plan.json` is rewritten as today
(tmp, rename, `store.rs:259-264`) plus a directory flush, carrying the journal
`seq` and digest it reflects. A failure before the append acknowledges
nothing; a failure after it is recovered by reducing the journal (§5.6);
`plan.json` may lag and is regenerated from the journal when its recorded
`seq` is behind. This inverts today's order at `ops.rs:535-541` (write, then
emit to a sink whose `record` returns `()`). The session sink
(`plan/ledger.rs:9-25`) keeps writing the slim `PlanOpRecord` as telemetry
and may fail without failing the op; the journal may not. A spawn is
journaled as `spawn_intent {todo, attempt, effect_id}` before
`delegate.spawn` (today the spawn precedes the write, `ops.rs:535-539`) and
`spawn_result {agent}` after; recovery lists an intent without a result as
`NeedsReconciliation` and never spawns for it. The record is `PlanOpRecord`
(`crates/types/src/plan/ledger.rs:12-28`) plus:

```json
{"plan": "ship-logrotate-lite", "op": "done", "actor": "main", "at": 1757650000000,
 "todo": "write the test suite", "from": "running", "to": "done", "todos": 6,
 "+seq": 41, "+requestId": "r-7f3a-41", "+expectedRevision": 12, "+attempt": 2,
 "+args": {"label": "write the test suite", "output": "local://report.json"},
 "+argsHash": "sha256:…", "+programHash": "sha256:…", 
 "+verdict": {"outcome": "pass", "score": 1000, "items": [["pytest", "pass"], ["report", "pass"]], "at": 1757649999000}}
```

`seq` is per root (a sub-plan's records ride its root's journal, so one
transaction covers both files) and monotonic; `requestId` is the caller's
idempotency key (a duplicate with the same `argsHash` returns the recorded
result; the same id with different args is refused); `expectedRevision` is
compared before any effect for non-commutative ops; `args` is the canonical
JSON of the `Op` (keys sorted recursively, array order kept, one UTF-8
serialization with golden vectors shared by Rust and Python), so `yi plan
repair` can reduce the log without executing anything; `argsHash` is sha256
(`sha2` is a workspace dependency, `Cargo.toml:54`); `programHash` is the
sha256 of `program.py` at the time of the op or null; `verdict` is present on
`done`, `done_refused` and judge records; `digest` chains to the previous
record. Record kinds that appear only in the journal: `done_refused` (§6.5),
`verification_requested` and `verification_stale` (§6.3), `spawn_intent` and
`spawn_result`, `import` (§5.5), `fuse_reset`, `reconciled`, `accepted_by_user`,
`program` (§8.3), `revoke` and `repossession` (§7.4). F3a landed no `judge` kind:
a juror's line rides the verdict of the `done` or `done_refused` record (§6.4).
The append itself, in `har-io`'s terms: one `write_all` of the
newline-terminated record, then `sync_data` on the journal file (the commit
point; a failure here acknowledges nothing and the partial line is the torn
tail recovery sets aside); then the checkpoint through `File::create_new` on
a temp file in the same directory, `sync_all`, `rename`, and `sync_all` on
the directory; the reader takes `Read::take(RECORD_CAP)` per line and treats
an over-long or unterminated record as damage, never as a shorter record.
`OpSink::record` returns `Result` for the journal sink; the telemetry sink
alone may swallow its error. Platform assumptions (a local filesystem with
`fsync`, one host, one writer under the lease, a lease that is never stolen
from a live pid) are named in the storage ADR; network filesystems and
cross-host writers are out of scope.
A record has a size limit and a transaction that would exceed it is refused
before any plan changes; the 32 KiB cap on `plan.json` is kept by moving long
notes, examples and source to artifact references, never by cutting.

### 5.4 `program.py`

An audit export. `plan.op{op: "program", cell_id, source_ref}` (F1a) records
a cell's source as an artifact before the first effect that cell causes
(§8.3); the kernel appends `# --- cell <id> <iso-time>\n<source>\n` to
`program.py` and the journal record carries `programHash`. A `supersede`
appends `# --- version <n>` before the next cell. Nothing rewrites the file,
nothing executes it (recovery is §5.6; `Plan.resume` reads durable state,
never source), and the JSON tool never touches it. A plan with no
`program.py` is a JSON-tool plan and completes without one
(`plan_walkthrough::every_fixture_completes_through_the_json_tool_with_no_program`).

### 5.5 Import of format 1 (explicit, lossless)

Import is an op, `Op::Import`, never a side effect of a read: a read of a
format-1 file returns the plan read-only with a notice naming the command
(`yi plan import <id>`, or the tool's `import` op). The importer
(`plan/import.rs`, new) reads and validates the old file; stores the original
bytes as an immutable artifact `artifacts/<sha256>` under the plan directory
(a blob the store never parses as a document; the no-Markdown rule is about
documents the engine reads); converts the frontmatter; maps `## <label>`
sections to `todos[].note` up to 4 KiB and larger sections to an artifact
reference, never a cut; keeps unknown fields under `extra` with provenance;
`Check::Command` becomes one critical `cmd` item marked `revalidate` (no
verdict is imported), `Check::Stated` a `LegacyUnverified` requirement kept
verbatim; todos already `Done` import as `LegacyUnverified` and stay visible
as history; the genesis record carries the original digest and the mapping;
`plan.json` is written last. The original `.md` is left in place until a
separate, user-confirmed cleanup; the reader (`split_frontmatter` :74,
`parse_document` :106, `DocumentError` :65) stays for two releases so a
skipped release still imports. Fixtures:
`crates/runtime/tests/fixtures/plans/format1/campaign.md` (what the campaign
walkthrough writes today), the expected `format2/campaign/plan.json`, the
expected artifact digest, and one file with a 6 KiB body section that must
land as an artifact reference.

### 5.6 Single writer, lease, and the user's authority

The lease is unchanged (`store.rs:320-348`, `.lease/` directory, pid or
30 s mtime staleness, RAII release). Deleted: `PlanStore::user_edits`,
`HandEdit`, `running_by` (`store.rs:143-150,376-392`),
`PlanEngine::fold_user_edits` and the `known` snapshot map
(`ops.rs:515,547-563`; `ops.rs:548-551`), `PlanFile.body` (`store.rs:140`).
The engine reads `plan.json` under the lease as the checkpoint of the journal
it names (`journal_seq`, `journal_digest`); a `plan.json` whose digest does
not match its journal is refused as `ExternalEditDetected` on a mutation and
regenerated from the journal on a read; an edited view never overwrites the
journal, and an explicit `import` of an edited view is a new generation with
provenance, never a verified status carried over.

`check_actor` (`table.rs:159-164`) becomes:

```rust
Actor::Owner => !matches!(op, OpKind::FuseReset | OpKind::Resolve | OpKind::Accept),
Actor::User(_) => true,                       // minted only by the confirmed path below
Actor::Host => matches!(op, OpKind::Unblock | OpKind::Reconcile),
Actor::Child(_) => matches!(op, OpKind::View | OpKind::Submit),
```

`Actor::User(Url)` (`ops.rs:27`) carries the citation, and the citation is
provenance, not proof: an agent's `bash` can run `yi plan fuse reset`, so
argv attests nothing. `Actor::User` is minted in exactly one place: the
host's confirmation path. A CLI administrative verb (`fuse reset`, a `repair`
resolution, `accept`) submits a request to the running session through the
console rpc (`crates/cli/src/rpc.rs:217-230` gains the action), which raises
it as a permission question (`Decision::Ask`, `gate.rs:120-124`) bound to
`{root, op, argsHash, expected_revision, expiry}`; the human's answer in the
console or TUI mints `Actor::User(user://<n>)` for that one request, consumed
once. With no session running, the CLI applies only non-administrative ops,
as `Actor::Owner`. The plan tool's actor stays fixed at construction
(`tool.rs:515-520`); `plan.op` fixes Owner or Child by the requesting kernel;
tests pin that no surface accepts an `actor` argument
(`plan_tool::the_tool_and_the_request_refuse_an_actor_argument`) and that an
agent-invoked CLI cannot reset the fuse
(`plan_ops::agent_cli_cannot_reset_fuse_as_user`). The op record's `actor`
field stores the citation verbatim.

Two new CLI verbs beside `lint` and `report` (`crates/cli/src/plan.rs:17-27`):

| verb | op | effect |
|---|---|---|
| `yi plan fuse reset [<plan>]` | `Op::FuseReset` (User only, confirmed in the session) | `Plan::reset_spawns()` (a second writer beside `charge_spawn`, `doc.rs:573`); the record carries the prior count; a supersede, a reconciliation or a kernel restart never lowers the fuse |
| `yi plan repair [<plan>]` | `Op::Repair` (Owner or User; resolutions User only) | reconstruction, then reconciliation: verify the journal's digests and sequence (a damaged record inside committed history stops with `RecoveryRequired`; only a torn final line is set aside with its bytes kept); reduce the journal to state with no effects; regenerate `plan.json`; then list every `Running{by}` whose agent has no live process and no result as `NeedsReconciliation` with the evidence found (a live process is reattached, a durable result reused, a proven-never-started intent may be re-issued under its own effect id); each resolution (retry as a new attempt, fail, accept) is a confirmed user op; a stale `.lease/` with a dead pid is condemned |

Every other op (`init … supersede`, `set`, `view`, `import`) is reachable as
`yi plan <op> [--json args]` as `Actor::Owner`; the CLI parses the same
argument shape as the tool (`tool.rs:344`), so one parser serves the three
surfaces (a test pins that every fixture step's args parse through the CLI
path too, `tool.rs:617` extended).

### 5.7 What gets deleted (store)

`fold_user_edits`, `known`, `user_edits`, `HandEdit`, `running_by`,
`PlanFile.body`, the frontmatter writer (`store.rs:259-260`), the plan
skill under `python/skills/plan/` and its `PYTHON_SKILLS` entry
(`bootstrap.rs:274`), the stale sentence in `python/skills/plan/SKILL.md:3-6`
with the file, the `FRONTMATTER_CAP_BYTES` name (renamed `PLAN_CAP_BYTES`,
same value). Net: `store.rs` shrinks by about 120 lines and grows by about
160 (schema validation, import, ops sink); `ops.rs` shrinks by about 40.

## 6. Contracts

### 6.1 The item list type (`crates/types/src/plan/contract.rs`, new file; schemas.lock `--update`)

The contract lives on the todo (`Todo.contract`), inline or delegated;
`Delegation` keeps execution and context. Wire fields are snake_case like the
rest of `TodoRepr`.

```rust
pub struct Contract {
    pub class: ContractClass,             // Writer | Reader | Inline | Service: picks the floor
    pub items: Vec<ContractItem>,          // 1..=16, unique ids
    pub threshold: u16,                    // 1..=1000, default 1000: every decided item passes
    pub min_coverage: u16,                 // 1..=1000, default 1000: every item decides
}
pub struct ContractItem { pub id: ItemId, pub critical: bool, pub weight: u16 /* 1..=100 */, pub decider: Decider }
pub enum Decider {
    Cmd { checker: ArtifactRef, timeout_ms: u64 },                 // a frozen manifest: command, cwd policy, protected files
    Schema { schema: ArtifactRef },                                 // validates this attempt's output artifact
    Example { cases: ArtifactRef, runner: ArtifactRef, timeout_ms: u64 },
    Judge { rubric: ArtifactRef, evidence: Vec<ArtifactRef>, policy: JuryPolicy },   // a jury of one or three (F3a)
}
pub enum ItemVerdict { Pass, Fail { detail: String }, Abstain { reason: String }, Escalate { question: String } }
pub enum Outcome { Pass, Fail, Abstain, Escalate }
pub struct VerificationToken { pub plan: PlanId, pub version: PlanVersion, pub todo: TodoLabel, pub attempt: u32,
    pub contract_digest: String, pub criteria_digest: String, pub output_digest: String,
    pub snapshot: String /* shadow-gitdir tree id */, pub integration: Option<u64> }
pub struct Verdict { pub token: VerificationToken, pub outcome: Outcome, pub score: u16, pub coverage: u16,
    pub items: Vec<(ItemId, ItemVerdict)>, pub elapsed_ms: u64, pub at: u64 }
pub fn aggregate(items: &[(ContractItem, ItemVerdict)], threshold: u16, min_coverage: u16) -> Outcome;  // pure
pub fn floor_of(class: ContractClass) -> Floor;                                                          // data
pub enum Resolution { VerifiedDone, AcceptedByUser, LegacyUnverified }                                  // on TodoState::Done
```

Ids and scalars are newtypes with checked constructors, never bare
integers or strings at a boundary: `AttemptId(u32)`, `RequestId`,
`EffectId`, `Seq(u64)`, `Digest` (32 bytes, hex on the wire), `Weight`
(`1..=100`), `Permille` (`1..=1000`) for `threshold`, `min_coverage`, score
and coverage; each derives the house line (`Debug, Clone, Copy, PartialEq,
Eq, Hash` where `Copy` is valid) and deserializes through `try_from`.
`aggregate` computes `1000 × pass_weight / decided_weight` with
`checked_mul` and a `NonZeroU64` divisor (the empty decided set is handled
before the division, rule 4); `attempt` and `refusals` advance by
`checked_add` and refuse at their caps. `Verdict`, `VerificationToken`,
`Lease`, `Receipt` and `Disposition` are `#[must_use]`: discarding one loses
the only evidence of an unfinished operation. `ArtifactRef` names an
immutable blob under `.yi/plans/<slug>/artifacts/` by digest, with media
type, length and provenance, written with `create_new` and read by digest; a
`local://` path handed to a builder is resolved and frozen into one at
`start`, so a criterion is never a mutable pathname. `Delegation.accept: Check` (`doc.rs:196`) becomes
compatibility input only (§5.5). `BlockedOn::External { probe }`
(`doc.rs:74`) is untouched; the probe ladder keeps running `sh -c` through
`goal::run_check`. The existing schema subset (`schema.rs:3-4`) ignores
unknown keywords: a criterion schema carrying an assertion keyword the subset
does not implement (`pattern`, `minimum`, `additionalProperties`, and the
rest) is refused at `start`; descriptive keywords (`description`, `title`,
`examples`) are allowed by list.

### 6.2 Aggregation and outcomes (pure, deterministic)

Given one verdict per item (a missing, duplicate or unknown id invalidates
the verdict):

1. any critical `Fail` → `Fail`;
2. else any `Escalate` → `Escalate`;
3. else any critical `Abstain` → `Abstain`;
4. else the decided set is the `Pass` and `Fail` items; empty → `Abstain`;
5. `coverage = ⌊1000 × decided_weight / total_weight⌋`; below
   `min_coverage` → `Abstain`;
6. `score = ⌊1000 × pass_weight / decided_weight⌋`; `score ≥ threshold` →
   `Pass`, else `Fail`.

Integer arithmetic in `u64`. A critical item never leaves the denominator
by abstaining (the original rule let one unavailable critical judge plus one
passing item pass, §0 R4). Floors (`floor_of`), checked by `Plan::validate()`
(`doc.rs:604`) at `init`, `append`, `retry`, `supersede`, and by `do_start`:

| class | floor |
|---|---|
| writer | ≥ 1 critical behavioural item (`cmd` or `example`); a schema may add shape; a judge item never stands alone |
| reader | ≥ 1 critical schema item (the findings shape); provenance checks for any grounded claim |
| inline | the writer or reader floor of its declared role; inline execution is not an exemption |
| service (F3c) | ≥ 1 critical health `cmd` plus a separate shutdown contract; health is not completion |

A deliverable with no adequate deterministic criterion goes to a judged
(F3) or user-accepted (`AcceptedByUser`) resolution; a trivial command added
to satisfy the floor is the failure MAST names (specification failures,
§15), and the fixture review looks for it. The rubric is frozen at `start`
(§6.3); a `done` whose contract or criteria digest differs is refused
`ContractDrift { label }`; the road back is `retry` (a new attempt with a new
freeze) or `supersede`. A `judge` item on a plan whose kernel is dead is
never a pass: a juror reads its evidence through `fetch`, so one that cannot
fetch cannot quote and abstains by the quote rule (F3a landed no `no kernel`
reason of its own). Before F3a every `judge` item was refused at declaration.

### 6.3 The done path, in order

```
plan.op{done, request_id, expected_revision}  (tool, host request, or CLI)
  1  apply: check_actor → lease → read the checkpoint → locate_step(Done) (ops.rs:809)
        → the todo's current attempt, its frozen contract and criteria, its output artifacts, and
          for a worktree todo its candidate disposition (§6.6) are all present, else refuse
  2  commit verification_requested { token, effect_id } to the journal (§5.3); a second done
        for the same token returns or awaits the same verification
  3  release the lease (drop)
  4  Verifier::run(contract, snapshot, token) on spawn_blocking, whole-verification deadline
        (DEFAULT_CHECK_TIMEOUT_MS 600 s, goal/mod.rs:23) and per-item deadlines:
        cmd     → run_check_in(workspace_of(snapshot), checker, timeout) under the session's permission
                  mode and cancellation (goal/mod.rs:46 gains a cwd; the probe keeps None)
        schema  → validate_product over the attempt's output artifact bytes (ops.rs:810-838, hoisted);
                  an unserved product or schema is a refusal, never skipped (today :836-838 skips)
        example → the runner over each case (§6.5)
        judge   → the jury of §6.4 (F3a); a path that seats no jury abstains the item
  5  re-acquire the lease, re-read, compare the whole token: attempt, version, contract, criteria,
        output digest, snapshot, integration generation
        mismatch → commit verification_stale, refuse; not a product failure, no refusal charged
  6  Pass and the token current → commit the verdict and the transition Done { VerifiedDone } as one record;
        a worktree todo passes through §6.6's acceptance first
     Fail | Abstain | Escalate → commit done_refused { verdict }, bump todo.refusals (an explicit
        event over the todo's life, which nothing reads for the cap), return the refusal with every
        item's line; this attempt's refused verdicts == DONE_REFUSAL_CAP (3) → the todo steps
        Blocked { on: User, note } as its own committed transition (the human inbox). The count is
        per attempt (`done.rs` refused_verdicts), so a retry opens a fresh one and RETRY_CAP is the
        only durable bound on a scheduler that retries a failed todo (§8.5). An Escalate outcome
        blocks on its own refusal without waiting for the count, and its note names the
        escalation rather than a refusal tally (F3a, §6.4)
```

The lease is released around step 4 because a command may run ten minutes
and every other op on the store would wait behind it; the token comparison at
step 5 makes the window safe (a todo retried, superseded or resubmitted
between 2 and 5 makes step 5 refuse as stale). The checker's process shape
(`har-threat`, `har-async`): `/bin/sh -c` by absolute path, `env_clear()`
plus an allowlist (`PATH`, `HOME`, `LANG`, `TMPDIR`, and the variables the
contract's manifest declares; today `run_captured` inherits the whole
environment, `process.rs:110-127`), an explicit cwd, its own process group
with `kill_tree` (`process.rs:31,123`), stdout and stderr drained and capped,
and one `Instant` deadline computed at step 2 and carried into the
`CancelFlag` (`goal/mod.rs:52`); a `spawn_blocking` job cannot be aborted, so
that flag is the bound. The `plan.op{done}` future may be dropped by the
kernel comm at any await; the effect committed at step 2 does not depend on
it, and its tasks live in a `JoinSet` the host shuts down rather than as
detached handles. The snapshot is the
shadow-gitdir tree the turn checkpoints already capture (`checkpoint.rs:16-27`,
`Checkpoints::capture` :39); a checker that also reads untracked or external
inputs declares them in its manifest or is marked non-reproducible in the
verdict. Infrastructure failures (a missing resolver, a timeout of the
verifier itself) are `Abstain` with the reason and are retried under their own
bounded backoff, never charged to the product's refusal count. Concurrent
`done` calls share the committed verification effect and cannot run the
checks twice or charge two refusals.

### 6.4 The judge child envelope (F3a; the shape is fixed here so F0c's `Abstain` stub has a target)

| field | value |
|---|---|
| spawn | `SubagentHost::spawn_seated` with `wall = { deny_write: ["."], deny_url: [every scheme but `local://`] }` (as landed: whole schemes, which needs no owner name and covers `agent://` and `plan://` too; `deny_url` is a prefix list, `deny_read` is paths), `isolation: none`, `check: none`, no `fork`, no `context`; cooperative, as every wall is, so what refuses a source that is not evidence is the quote check |
| model | the registry (`subagent/models.rs`, where `find_models` now lives) filtered to a recorded model identity whose vendor segment differs from the owner's (`openrouter/z-ai/glm-5.3-flash` → `z-ai`), a weak heuristic for independent errors, stated as such; as landed the owners are the host's own model and the selector the plan spawned the todo's child with, the candidates are models on the host's provider or on one whose key is set, cheapest input first; none available → `Abstain { "no other family" }`, never the owner's model |
| brief | the rubric, the evidence artifact ids, the fixed schema, and nothing else: no owner transcript, no author, no prior verdict; evidence is served through `fetch` under the wall, never inlined |
| schema | `{"verdict": "pass\|fail\|abstain", "reason": string, "quotes": [{"url": string, "line": integer, "text": string}]}`, parsed strictly as `JurorAnswer` from the retired juror's last answer (as landed: `rlm.result` is the kernel's road and would promote the answer into the owner's transcript, so the jury reads the record itself); anything else abstains, and so does a decided vote that quotes nothing (`auto_review.rs:62-81` is the pattern) |
| quote check | every quote's `url` must be one of the item's evidence addresses and be backed by the judge's own fetch log (`fetch::rows_of` over its transcript) at the frozen artifact's hash, and its `text` must match that line, which a blank line never does; any miss → the item is `Abstain { "unbacked quote ..." }` and the dropped juror's line carries the check's own `unbacked` flag, never a prefix of the reason the juror wrote; as landed the hash is the whole file's, so a fragment or paged read backs nothing; a matching quote proves provenance, not entailment |
| jury | `policy.n` children in parallel with a predeclared quorum: for n = 3, at least two decided votes, two `pass` → `Pass`, two `fail` → `Fail`, anything else `Abstain`; n = 1 is labelled single-judge; pass and fail quorums are validated disjoint for other n |
| bounds | `JUDGE_CAP_PER_TODO` 3 per plan version, then `Escalate`, which blocks the todo on the user at once; as landed the count is derived from the journal's `verification_requested` records and there is no `todo.juries` field; a juror draws a lease from the owner and counts against the family cap 16; it sits above the parent cap 8 and the depth limit under the `Purpose::Verification` permit the done path reserves, one jury at a time, so eight retained workers cannot starve their own judges (§7.6) |
| record | one line per juror with the model identity, the vote, the reason and whether the quote check dropped it, carried as `jurors` on the item's line of the verdict the `done` or `done_refused` record journals (as landed: no `judge` record kind of its own); the item verdict aggregates in Rust; calibration (false accepts, false refusals, coverage, cost) on an independently labelled set precedes default use |

### 6.5 Every completion path, the `done_refused` record, the examples runner

- `done` requests verification (§6.3); it never sets `Done` itself.
- `set` (`ops.rs:1007-1041`) parses declarations and requested transitions
  and hands each transition to the same validator; a row asking for `done`
  needs a matching committed verdict or it is refused; a row may still add,
  reorder, block or abandon.
- `import` marks legacy success `LegacyUnverified`; `repair` reconstructs
  and cannot manufacture a pass; `supersede` starts a new generation,
  invalidates outstanding tokens and keeps prior outcomes in history; the
  CLI and Python carry no actor; a restored or edited `plan.json` never
  overwrites the journal (§5.6). Every new op passes the same invariant
  tests before it may write task state.

`done_refused` is a journal record with no step-table row: it never changes
a todo's state, so it cannot appear in `STEPS`; the engine appends it with
the verdict and returns `PlanOpError::Refused { label, verdict }`; the
refusal counter bump is its own explicit event. `extract.py` counts it from
the record (`done_refused` signal, §10.6). The examples runner: `cases` is an
artifact holding a JSON array of `{"input": <json>, "expected": <json>}`;
`runner` is a frozen checker manifest run once per case in the verification
workspace with the case's `input` serialized on stdin; stdout must be one
complete JSON value, compared structurally (key order and whitespace do not
fail; numbers by a documented policy, tolerances only as frozen criteria);
trailing prose, several values, non-finite numbers, timeouts and output
overflow are distinct failures; the item's `detail` lists failing case
indexes; all cases run under the item's and the whole verification's
deadlines. No per-language harness (ponytail: a `python3 solve.py` runner
covers the corpus; a language-aware runner is a later row if a rollout needs
one).

### 6.6 Worktrees: candidate, integration, cleanup

A worktree todo has four phases, tracked by attempt and artifact ids, never
by whether `Option<Lane>` is `Some` (`take_settled_worktree` takes the lane
on merge and on discard alike, `lane/mod.rs:1033-1052`):

| phase | required before the next |
|---|---|
| execute and submit | the child quiescent (no running command, `take_settled_worktree` :1043-1047), a candidate commit on its branch (`merge_into`'s incident rule, :939-948, hoisted into `Lane::settle()`), immutable output references |
| verify the candidate | a passing verdict on the candidate tree (§6.3 with `snapshot` = the candidate commit) |
| prepare and verify the integration | under a short integration lock: pick the parent generation, prepare the merge in a staging worktree (never the user's dirty checkout), run the required checks against that exact tree outside the lock |
| accept | publish only if the parent generation still matches (else prepare again); commit the acceptance record `{candidate, parent_base, integrated, verdicts, conflict_provenance}` with the transition Done |
| reject, fail, cancel | an explicit disposition, `Retained`, `Discarded`, `MergeFailed`, recorded before the lane and slot are released; retryable by id; never a merge to free a slot |

`done` on a worktree todo is legal only at the accept phase; `fail`, `drop`,
`supersede` and revocation take the disposition path, so `step_todo`'s reap
on every exit from Running (`ops.rs:699-708`) keeps working and a failed
branch can be kept or discarded without merging. Ordinary cleanup cannot
erase the only copy of a result: the branch name or a `history://` pin lands
in the disposition first. One candidate at a time in F0d; a tournament (a
later shape, §8.5) verifies each candidate separately and accepts one
winner's integration.

## 7. Mailbox and leases

### 7.1 The envelope

A message between family members is already an `AgentMessage::Custom`
with `custom_type: "agent_message"` (`mailbox.rs:63-70`); its `details` is
`None` there (:68). The envelope rides in `details` (`message.rs:241-242`); the
`custom_type` does not change, so nothing that counts `agent_message` entries
today moves.

```json
{"id": "main-17", "from": "main", "to": "tests", "kind": "request",
 "conversation": "main-17", "inReplyTo": null, "seq": 17, "sentAt": 1757650000000,
 "deadlineMs": 1757653600000, "body": "…≤ 16 KiB…", "ref": "family://findings-tests"}
```

Rust (`crates/types/src/mail.rs`, new; schemas.lock `--update`):

```rust
pub struct Envelope { pub id: MailId, pub from: String, pub from_incarnation: Option<u32> /* None: no service; F3c */, pub to: String,
    pub to_incarnation: Option<u32> /* None: whoever holds the address */, pub kind: Kind,
    pub conversation: MailId, pub in_reply_to: Option<MailId>, pub seq: u64, pub sent_at: u64,
    pub deadline_ms: Option<u64>, pub body: String, pub reference: Option<Url> }
pub enum Kind { Inform, Request, Reply, Progress, Failure, Cancel }
pub struct Receipt { pub id: MailId, pub state: Delivery }        // Delivery: Queued | Woken | Inboxed
```

### 7.2 Kinds

| kind | sender | receiver behaviour | wakes an idle receiver |
|---|---|---|---|
| inform | any | queued on the steer queue (`hooks.rs:68-75`) | yes, through `heartbeat_hook` (:77) |
| request | any | queued; the sender may `await` the reply (§7.3) | yes |
| reply | any | resolves the waiting request's future; queued for the transcript | yes |
| progress | child | written to the inbox and to `status().note`; never queued as a turn message | no |
| failure | child, or the host on repossession | queued; `wait` reports the child as `failed` | yes |
| cancel | parent, or the host on revoke | queued; the child's loop sees a cancel flag at its next message boundary and ends its turn with a `reply` naming what it kept | yes |

Today's `followup: bool` (`mailbox.rs:87-91`, `rlm/__init__.py:343`)
maps to `kind: inform` with `FollowUp` delivery; it stays as a spelling.

### 7.3 Receipts, order, the inbox, references, request/reply, backpressure, query

- **Receipts.** `agent_message.send` returns `{id, state}` with `state` one
  of `inboxed` (persisted in the receiver's store, the receiver not running),
  `queued` (persisted, and on the receiver's steer queue mid-turn), `woken`
  (persisted, and a turn started). Persisted first, always: a receipt says
  the host accepted the envelope, not that a model read it or acted. No
  exactly-once promise is made (§3.4); re-presentation is an explicit action
  with the same message id.
- **Per-pair order.** `seq` is allocated per `(sender, recipient)` binding at
  enqueue time by `SubagentHost` and presented in that order; unrelated
  senders are never sorted into one fictitious sequence. Erlang's guarantee,
  no more. `seq` is not the receiver's history cursor.
- **Durable inbox.** Before any delivery the host appends the envelope to
  the receiver's session store as `Entry::Custom { custom_type:
  "agent_message", data: envelope }` (`append_custom`,
  `crates/session/src/store.rs:185`); presentation state (`presented_at`) is
  a second record, so a crash between the two leaves the item inspectable
  and unpresented rather than lost or shown twice. A member reads its inbox
  with `history://<self>/since/<seq>/custom/agent_message` (the resolver
  already parses the segments `tail/N` and `since/S`, `fetch/schemes.rs:30-44`,
  D165; `custom/<type>` is one more segment over `EntryQuery.custom_type`,
  `crates/session/src/query.rs:14`). No new syscall.
- **Body cap.** `CONTEXT_TOTAL_CAP` 16,384 (`mailbox.rs:16`) bounds `body`;
  a larger payload is refused with "put it and send the artifact id"
  (`rlm.put`, D164, or an artifact reference). Refused, never trimmed.
- **request/reply.** `await rlm.request(target, text, timeout=300)`
  allocates the conversation and installs the waiter (a `tokio::sync::oneshot`
  keyed by conversation id, with the intended respondent and its incarnation)
  before the request can reach the receiver; only a `reply` from that
  respondent with that `inReplyTo` resolves it; timeout or cancellation
  retires the waiter exactly once, and a late reply stays in history without
  resolving anything new; a parent restart lists outstanding conversations for
  explicit resume, never pretends the in-memory waiter survived. The receiving
  model replies with `rlm.send(target, text, reply_to=<id>)`; the affordance on
  the delivered request names that call.
- **Backpressure.** Bounded body bytes, outstanding requests per sender,
  inbox growth, and active waiters; `progress` envelopes are coalesced and
  never start a turn; control kinds (`cancel`, `failure`) have reserved
  capacity so a progress flood cannot delay a cancel.
- **query.** Reading without a turn is `rlm.fetch("kernel://<agent>/<var>")`
  (D164) and `rlm.status(name)` (D165). Nothing new; `help(yi)` names
  them as the query primitive.

### 7.4 Leases: drawn, reserved, revoked

```rust
pub struct Lease { pub holder: String, pub parent: String, pub deadline_ms: u64,
    pub tokens: Option<u64>, pub granted_at: u64, pub revoked: Option<Revocation> }
pub struct Revocation { pub at: u64, pub grace_ms: u64, pub reason: String }
pub struct Repossession { pub lease: Lease, pub at: u64, pub kept: Vec<Url>, pub disposition: Disposition }
```

- **Deadlines are inherited bounds, not currency.** A requested deadline past
  `parent.deadline − cleanup_grace` is refused with both numbers (never
  silently clamped); an omitted deadline takes the documented inherited
  default; an already-expired parent refuses the spawn. `wiring.deadline`
  (`environment.rs:141`) is the root's clock and a child's environment shows
  its own `deadline:` line (`environment.rs:107`).
- **Tokens are reserved at admission.** `SpawnSpec.budget` (`doc.rs:177`)
  is reserved atomically against the parent's available budget (available =
  remaining − outstanding reservations); usage is debited as turns settle,
  reconciled at completion, and the unused reservation returned once at
  reap (`mailbox.rs:491`); unknown usage (a turn whose provider reported
  none) is an explicit uncertainty that blocks the return. Retries, judges,
  inline work and grandchildren draw from the root's one budget. The
  exokernel's rule: expose allocation, refuse over-allocation, hide nothing.
- **revoke(grace).** `rlm.revoke(name, grace_s=30, reason)` commits
  `Revocation` on the lease, sends `kind: cancel` bound to the child's
  incarnation, stops admitting new work for it, and registers the grace's due
  time with the deadline scheduler: the probe loop (`probe.rs:244-260`)
  gains a `tokio::sync::Notify` so an earlier due time interrupts its sleep
  (today `next_wake` caps the sleep at 60 s and a due time registered
  mid-sleep waits it out, :233-241; slow probes run off the same tick but
  never delay a cancel). At expiry the host `interrupt`s (`subagent.rs:922`),
  kills the child's process group and **joins** it (`abort()` and a signal
  are requests, not barriers, `har-async`; nothing below runs until the join
  returns), settles its branch (`Lane::settle()`, §6.6), commits the
  repossession record with the disposition and the kept references
  **before** the child's record and reservations are released, and sends the
  parent one `failure` envelope naming it. If stopping,
  settling or committing fails, the child stays in a visible
  `RepossessionPending` state with its references kept; a clean termination
  is never reported while a process can still write. The cancel latency is
  observed and reported (tests and live telemetry), never inferred from the
  timer's minimum sleep.
- **on_parent_close.** `SpawnSpec.parent_close: Terminate { grace_ms } |
  RequestCancel`, default `Terminate { 30_000 }` with preservation
  (`RequestCancel` is a cancel envelope plus the same bounded termination if
  the child does not stop). `Abandon` is refused until a durable supervisor
  owns an orphan's address, budget, inbox, artifacts and deadline (F3c at the
  earliest). What happens today when a root session ends with children
  running is not pinned by any test the recon found (speculation: the host
  drops with the session and the children die with their kernels); F2b's
  first test pins the current behaviour before changing it
  (`recursion_e2e::children_at_parent_close_today`).

### 7.5 Wake: lifecycle notices, the stuck notice, `wait` with a cursor

- **Notices wake through `wake_idle_hook`.** `wiring.rs:603` changes from
  `session.notice_hook()` to a wrapper over `session.wake_idle_hook()`
  (`hooks.rs:82-96`): it waits out a running turn and starts one when the
  session is idle, which is what a status-gated hook cannot do at `AgentEnd`
  (the status is still Running there, as the hook's own comment records).
  The delivery state is written first (the notice is a `custom{agent_message}`
  entry on the parent, §7.3); the wake is a signal that work exists, never
  the only copy of it, so an ignored `run(message)` error loses nothing.
  `notice_hook` (:172-179) keeps its other callers (`session.rs:209`;
  `wiring.rs:324,525,567,650`: restore and store notices that must not start
  a turn). Wakes coalesce; distinct updates do not. Tests cover the parent
  idle, mid-turn, at `AgentEnd`, two children finishing together, and a
  parent restart (F0a).
- **The stuck notice.** The probe loop (`probe.rs:244-260`), with the
  `Notify` of §7.4, gains a second job with its own due time: for each running
  child compute `state_from_records` (`family.rs:136-160`); a child that is
  `Stuck` and was not stuck at the previous tick sends one `failure`-kind
  notice `[child <name> stuck: <note>]` through the same wake, latched until
  its records move. Deterministic, from the records the loop already writes
  (D165); one notice per episode.
- **`wait` takes a cursor.** `rlm.wait(timeout, cursor=None)` (`mailbox.rs:286`)
  stops draining a shared counter (`take_pending` :304-319 zeroes it, so two
  waiters steal each other's updates) and returns `{cursor, changed: [names],
  states: {name: running|finished|failed|needs_you|stuck}, notes: {name:
  note}}` computed from `states()` (`subagent.rs:719`) against the caller's
  last cursor; a caller arriving after a completion sees the terminal state
  at once; `updated` is kept one release as an alias of `changed`. The
  Python `RLMSpawnHandle.result` (`rlm/__init__.py:52-83`) stops polling
  `list_subagents` every 0.5 s and awaits `wait` with its own cursor (the
  `ponytail:` comment at :81 names exactly this upgrade); its timeout and
  missing-child behaviour are preserved.

### 7.6 Capabilities: walls on the spec, hereditary shrink, holds compiled down, capacity

- `SpawnSpec.wall: Option<WallSpec>` (`doc.rs:165-179`; `Wall`
  `wall.rs:10-14`) and `kwargs_of` (`dispatch.rs:84-107`) passes
  `deny_write`, `deny_read`, `deny_url` (the kwargs `subagent.rs:273-282`
  already accept). A plan-dispatched reader is finally walled (D166's rule 2
  becomes enforceable for engine spawns). Every wall is cooperative
  (`wall.rs:114-115`): it stops an honest agent at the mediated seams
  (`tools.rs:231`, `wiring.rs:193`, `tools.rs:199`), not native code; the
  plan claims nothing more.
- **Hereditary shrink** (hive L1, `2026-09-01-hive.md:410-414`): a child's
  effective wall is the union of its parent's wall and its own, with paths
  and URL prefixes canonicalized at the same seams that enforce them; a child
  may never spawn with a wall smaller than its parent's. Enforced in
  `SubagentHost::spawn` (`subagent.rs:470`) from the parent's
  `session.wall()` (`session.rs:339`).
- **Holds compile down** (hive L4, :427-431): a permission `Ask`
  (`Decision::Ask`, `crates/runtime/src/gate.rs:120-124`) is a question; a
  child that cannot reach a human cannot ask one. At spawn the child's
  permission mode is compiled: attached (its parent can reach the root's UI
  within the depth) → an `Ask` becomes a `request` envelope to the parent
  carrying the originating principal, operation, arguments and attempt,
  answered by the parent's own broker or forwarded up; detached (any child
  whose parent cannot forward) → `Ask` becomes `Deny` with the hazard text.
  If the compiled mode would deny the child's own brief (its `context` names
  a path the wall denies), the spawn is refused with that as evidence.
- **Capacity is accounted per resource.** Worker slots (8 per parent, 16
  per family, `subagent.rs:18,20`), retained worktrees (3 lane slots per
  repo shared with roots, `lane/mod.rs:17`), inline activities, verifiers,
  concurrent model requests and mailbox capacity are separate counters;
  verification capacity is reserved (or a worker's execution slot is released
  once its candidate is preserved) so eight retained workers cannot occupy
  every slot their own verification needs; the reservation still counts
  against the root's spend limits. The baseline numbers (width
  `clamp(cores − 1, 1, 8)`, 8, 16, fuse 64, retry cap 8, 3 lane slots) stay
  until a measured change is approved (§10). The spawn fuse is charged once
  per committed `spawn_intent` and a retry of the same intent is not charged
  again; nothing lowers it but the confirmed `fuse reset`.

## 8. User space: the `yi` library

### 8.1 Layout (`python/yi_runtime`, the wheel the bootstrap already installs, `bootstrap.rs:265`)

```
python/yi_runtime/pyproject.toml         packages = ["src/rlm", "src/yi"]
python/yi_runtime/src/yi/__init__.py      help(yi): the map; re-exports Plan, contract, cmd, schema, example, judge, shapes, roles, mail
python/yi_runtime/src/yi/plan.py          Plan, Todo handles, the plan.op transport, idempotency, program persistence
python/yi_runtime/src/yi/contract.py      builders that emit the yi-types Contract JSON (§6.1)
python/yi_runtime/src/yi/shapes.py        pipeline, fork_join, map_reduce, tournament, pod, scatter; Restart
python/yi_runtime/src/yi/roles.py         Reader, Writer, Judge specs → SpawnSpec + wall; verify_quotes
python/yi_runtime/src/yi/mail.py          send, request, inbox over rlm (F2a)
python/yi_runtime/src/yi/recipes/         fan_out_readers.py, writer_with_check.py (templates)
python/yi_runtime/tests/test_yi_*.py      unittest, run by the gate (check_guardrails.sh:36)
python/yi_runtime/tests/programs/*.py     the walkthrough fixtures as programs (F1b)
```

`python/skills/plan/` is deleted (its four host requests exist nowhere,
§2 A14). The `goal` skill stays.

### 8.2 Handles and idempotent todos

```python
from yi import Plan, Writer, Reader, contract, cmd, schema, example, shapes
plan = await Plan.create("ship logrotate-lite with a packaged tarball", request_id="create-01")
freeze = await plan.todo(key="freeze", label="freeze the CLI surface",
    accept=contract(cmd("python -m logrotate --help | grep -q -- --size", critical=True)))
tests = await plan.todo(key="tests", label="write the test suite", after=[freeze],
    delegate=Writer(isolation="worktree", deny_write=["docs/"]),
    accept=contract(cmd("pytest -q tests/", critical=True),
                    schema("local://.yi/schemas/test_report.json", critical=True)))
run = await plan.run(shape=shapes.fork_join, budget="2h")
run.outcome   # verified_success | failed | cancelled | unresolved | accepted_by_user
```

- Every method is one `plan.op` host request (§4.3) with a `request_id`
  the library mints per call (a retried call reuses it, so a duplicate is
  the same committed result); the library adds no state the store does not
  hold. `Plan.create(goal)` never guesses that a plan with similar text is
  the same run; `Plan.attach(plan_id)` joins an existing plan;
  `Plan.resume(plan_id)` reattaches a run (§8.3).
- **Idempotent `todo`.** Within a plan generation a stable `key` resolves
  to one todo; the label is display text and a lookup alias. Repeating a
  declaration with the same canonical dependencies, execution spec, contract
  and artifact identities returns the handle and writes nothing; a different
  declaration raises `SpecDrift(key, diff)` and writes nothing: a re-run cell
  must not silently rewrite a running todo. A label change is an explicit
  rename. The road on drift is `todo.retry(delegate=…)` (a new attempt after
  a legal failure or cancellation) or `plan.supersede()`.
- Builders (`cmd`, `schema`, `example`) resolve their `local://` inputs
  through the host into frozen artifact references at `start` (§6.1); the
  convenience syntax never leaves a mutable path inside a contract.
- `Todo` methods: `start()`, `done(output=None)` (raises `Refused(verdict)`
  on a `done_refused`, `Stale` on a stale token), `fail(cause)`, `block(on,
  note)`, `unblock()`, `retry(delegate=None)`, `decompose([...])`,
  `state()`, `attempt`, `child` (the agent name), `result(schema, timeout)`
  (the child's `rlm.result`), `submit(artifact)` (an inline attempt's
  product), `cancel()`.

### 8.3 Bounded and detached runs, cancellation, inline todos, source recording

- **Bounded.** `await plan.run(shape, budget)` blocks the cell until the plan
  finishes, the shape aborts, or the budget (wall clock; a token budget when
  the goal carries one) expires; the return names which. One scheduler lease
  per plan: a second `plan.run` attaches to the running one or refuses with
  its id.
- **Detached.** `plan.run(shape, budget, detach=True)` returns a `Run` handle
  and schedules the scheduler as an asyncio task in the owner's kernel; the
  task keeps stepping between cells (the kernel's loop runs while idle, which
  is how `bash()` handles already progress, `rlm/__init__.py:692`), and the
  host wakes the owner's turns through the dispatch follow-ups and the
  lifecycle notices (§7.5). Experimental until kernel restart, idle-loop
  stepping and cancellation are exercised on a real kernel (F1a's T2 tests);
  `run.status()`, `run.stop(scope=scheduling|cancel_active)`: the default
  stops scheduling, requests bounded cancellation of active attempts, and
  reports what remains.
- **Cancellation.** `todo.cancel()` requests the engine transition and the
  executor's cancellation; the todo is marked only after what the attempt did
  or preserved is known (an inline coroutine is cancelled and its partial
  artifact, if submitted, kept; a child is `rlm.interrupt`ed and takes the
  disposition path, §6.6).
- **Inline todos.** `plan.todo(key, run=coroutine)` executes in the owner's
  kernel as a thread of the plan with an ordinary attempt id: `start`
  records `Running{by: main}` (`ops.rs:775`); the coroutine's return value is
  submitted as an artifact with a host-minted id (`<plan>/<key>/<attempt>` or
  a content digest; a label is never a blackboard filename, `rlm.put`
  requires a file-safe token, `rlm/__init__.py:535-536`) and `done` runs the
  contract over those bytes; `family://` sidecars stay a coordination view.
  No admission slot (`table.rs:231-232`), but bounded execution and
  verification capacity (§7.6).
- **Source is recorded, never replayed.** Before the first mediated effect a
  cell causes, the library records the submitted cell's source as an artifact
  with a stable cell id (the cell text the kernel executes, taken at
  submission, not a later `In[-1]` lookup) and binds it to the plans it
  touches through `plan.op{op: program, ...}` (§5.4); a cell that later fails
  is still recorded as failed. Recovery never executes saved source
  (imports, closures, file writes, network, randomness and prior notebook
  state cannot be made safe by an idempotent `todo`; §0 S5). `Plan.resume`
  reattaches to durable state, reuses accepted results, reconnects live work,
  and exposes uncertain attempts for a decision; an external side effect with
  an unknown outcome is a reconciliation, never an automatic retry.

### 8.4 Recipes and instances

A recipe is a function in `yi/recipes/` versioned in the tree (a change is
a PR with a row); a program is the cell text that ran for one plan, kept in
`program.py` and never edited (§5.4). SoL-Pi's finding: disposable loop
templates beat a growing coordinator (§15). The disposable-instance rule:
`program.py` only grows; a change of mind is `plan.supersede()` and a new
version section; a recipe never reads a program.

### 8.5 Shapes: two ship in F1b, four are specified as prerequisites

Each shape is an `async def` over `plan` with a `Restart` strategy
(`one_for_one`, `rest_for_one`, `one_for_all`; `max` restarts within
`window`, OTP's intensity), a geometry check run before the first `start`
(refused with every problem named at once), and a scheduling order (policy,
§3.3). The kernel refuses over-admission (`table.rs:227-237` becomes a
predicate `admit(plan, label) -> Result<(), Refusal>` with the count); the
shape waits on `rlm.wait` with its own cursor and retries the refused
`start`. A restart is a new attempt after a legal failure; interrupting a
process does not make `drop` legal on a Running todo, and `Done` is not a
legal input to `retry` (`table.rs:38-141`), so every shape is tested against
the real step table beside its library mocks. Geometry does not prove
isolation: writers get worktrees, readers declared output artifacts.

| shape | ships | geometry check | order | restart | done by |
|---|---|---|---|---|---|
| fork_join | F1b | ≥ 2 ready todos, each a worktree writer or a reader | Vec order, up to the admission count | one_for_one, bounded | per child, through §6.3 and §6.6 |
| scatter | F1b | ≥ 2 readers bound to disjoint partitions, one lead | rounds | none: a failed reader is dropped from the round | the lead commits (§8.6) |
| pipeline | later; needs downstream attempt invalidation | edges form one path | the path | rest_for_one | the owner runs `done` per stage |
| map_reduce | later; needs immutable producer outputs and a schema-validating reduce activity | N maps with one contract, one reduce `after` all | maps, then reduce | one_for_one on maps | the reduce is an inline todo |
| tournament | later; needs per-candidate verification, one integration winner, legal loser cleanup | N todos with identical contract on one goal | all at once | none | the first passing candidate's acceptance |
| pod | F3b, as the recipe `yi/recipes/review_pod.py`; a calibrated findings contract is still owed | ≥ 2 first passes plus one arbiter `cmd` todo | passes, then the arbiter | one_for_one, bounded (`_schedule`'s); one_for_all on the passes is still owed | the arbiter's cmd |

Each later shape is its own PR with the prerequisite landed first and a
failure it fixes named in the row.

### 8.6 Scatter

Readers bound to stable partitions (`roles.Reader(partition=…)`, which builds
`deny_write=["."]` and passes a caller's own `deny_read` through; a wall is
cooperative, §7.6, and `deny_read` is paths where a partition is urls, so what
actually binds a reader to its partition is the quote seam below, not its wall),
a lead (the owner, or one writer child), rounds: the lead asks a question; each
reader answers `{"answer": str | null, "quotes": [{"url", "line", "text"}]}`;
a null answer is an abstention and is dropped; `roles.verify_quotes` fetches
each cited `url` through `rlm.fetch` and reads line `<line>` from it, pins the fetched digest, and drops
any quote citing a url outside the reader's own partition, any quote whose text
does not match and any answer whose every quote was
dropped; the lead sees only what survived; rounds continue until the lead
commits (the lead function returns `{"commit": answer}`) or `SCATTER_MAX_ROUNDS` (3, a lever). A
verified quote proves provenance, not entailment: an answer with no
supporting quote abstains, and correctly copied irrelevant text does not
validate a claim; the lead's commit is its own reasoning. ParSer's shape:
reading decoupled from reasoning, readers cheap (the cheapest model
`find_models` offers), only the lead reasons; its reported gains came with a
trained lead and a specific cache deployment (§15), so F1b measures scatter
against direct retrieval on the same tasks before it is recommended. For
judges (F3a) the same quote check runs in Rust against the fetch log (§6.4).

### 8.7 `help(yi)` and the prompt gates

Every public name carries a docstring whose last paragraph is one runnable,
awaited example; `python/yi_runtime/tests/test_yi_help.py` asserts it for
every name in `yi.__all__`. The three gates: `check_prompt_examples.py`
derives module prefixes from `python/yi_runtime/src/*/__init__.py` once its
glob widens from `rlm/__init__.py` to `*/__init__.py` (one line, :11-12);
the name allowlist in `crates/runtime/tests/ext_e2e.rs:350-359` gains
`api("python/yi_runtime/src/yi/__init__.py", "yi.")`, `plan.` and `shapes.`;
the byte budget moves once, at F1d, by `--update` in its own commit named in
the row (D137). Prompt bytes for documentation: none in the cached prefix;
the working model in `orchestrate.md` names `help(yi)` and two examples, and
help text still costs context when a model reads it, so the docstrings stay
short and the examples runnable. Synchronous names (`put`, `get`, `ls`,
`bash`) get synchronous examples; the gate checks each name's own kind.

### 8.8 The kernel-dead path

With no kernel, the model has the `plan` tool with every op (`tool.rs:523`),
the todo tool, and `orchestrate.md`'s JSON-tool example; the store, the
contract at done, the wake and the ledger are all kernel-side, so nothing in
F0 depends on a kernel. F1-F4 add the rich path only.

## 9. The procedural graph

### 9.1 Location and format

An F4 experiment, not a commitment: it earns its place only after the
byte-identical migration (§9.7) and a comparison of the current strings, a
plain rule table and the graph on the same tasks; the paper's renderer is an
online guidance model, Yi's is a deterministic function, and no number
transfers (§15). `crates/runtime/src/prompts/graph.json`, `include_str!`
into `affordance.rs` (frozen online: the shipped binary carries one
version); a `Graph` type in `crates/types/src/graph.rs` (schemas.lock
`--update`), `version: u32`.

```json
{"version": 1,
 "nodes": [{"id": "read", "kind": "tool"}, {"id": "plan.done", "kind": "op"}, {"id": "rlm.wait", "kind": "request"}],
 "edges": [{"from": "plan.done", "relation": "after_refusal", "to": "plan.retry",
            "condition": "done_refused", "guidance": "retry with a new delegation, or add the item the verdict names",
            "pitfalls": ["a second done with the same contract is the same refusal"], "weight": 80}]}
```

Nodes are Yi's verbs: every registered tool name, every plan op as
`plan.<op>`, every host request. Edges are typed
`(procedure, relation, procedure)` with a condition, guidance and pitfalls
(Procedural Graphs §15). Relations: `then`, `instead`, `before`,
`after_error`, `after_refusal`.

### 9.2 Conditions are a closed predicate set

`Predicate` in `crates/types/src/graph.rs` (as landed: a string parsed against
the closed table `PREDICATES`, plus the states the migrated producers branch
on, F4a), evaluated on facts the renderer already
holds at the seam (`tools.rs:305-308` appends the lines): `always`,
`result_ok`, `result_error(ToolErrorKind)` (`event.rs:177-185`),
`output_capped`, `todo_open`, `plan_ready_nonempty`,
`child_state(MemberState)`, `blocked_on(user|child|external)`,
`worktree_unmerged`, `done_refused`, `inbox_nonempty`. No model evaluates a
condition; the closed set is what makes the graph a trigger, not a judge.

### 9.3 Localization and rendering

Localize by exact match on the last tool call's name (plus the op for the
`plan` and `todo` tools); collect edges up to two hops whose conditions hold;
sort by weight; render `next: <guidance>` lines, dedup by guidance text, up
to the cap each producer keeps today: two for a tool result
(`affordance.rs:5-6`), three for the todo tool (`NEXT_LINES`,
`todo/text.rs:6`). Two hops is the paper's sweet spot (up to 70.9 percent
fewer tokens than the full graph, §15); the caps keep a result's tail the
size it is today.

### 9.4 Structural checks (a test, and the refiner's first gate)

Every node names a registered tool, op or request; every condition parses;
no self edge; ≤ 8 out-edges per node; guidance ≤ 160 bytes; pitfalls ≤ 3 of
≤ 120 bytes; ≤ 400 edges in the graph (refused, never trimmed); every
edge's `to` is reachable from some tool node. `crates/runtime/tests/affordance.rs`
gains `the_shipped_graph_passes_every_structural_check`.

### 9.5 The offline refiner (`evals/graph/refine.py`, stdlib)

Input: a proposals file (JSONL of edits: `add_edge`, `drop_edge`, `reword`,
each with the full edge and a one-line rationale) authored offline by a
model or a person; this is an envelope: enumerated edit kinds, a fail-closed
default (an edit that fails any structural check is rejected with the
check's name), a recorded verdict per edit, and a bound (`MAX_EDITS_PER_RUN`
20). For each surviving edit: build the candidate graph, run the training
slice (`evals/journeys/ab.py` for the faux-driven journeys; the paid slice by
the user's hand for T3), score with `evals/axes.py`, then the held-out slice
once.

### 9.6 The held-out gate, ties accepted, rejection memory

An edit is promoted into `graph.json` when the held-out pass count is not
lower than the current graph's (ties accepted, the paper's rule) and the
token total per solved task is not higher (as landed, F4b: the fit split
filters first, so an edit that drops a development pass is rejected as
`fit_passes_dropped` and never spends a held-out run, and a tie in which
neither graph solves anything prices no token, so §9.8's rule stands alone
there and rendered bytes that bought no pass are `guidance_bytes_unpaid`);
else it is appended to
`evals/fixtures/graph/rejected.jsonl` as `{editHash, baseVersion, protocol,
model, reason, scores, at}`; the refiner refuses the same edit against the
same base graph, protocol and model configuration, and treats an older
rejection as evidence, not a ban, once any of those changed. Repeated
validation decisions are selection (§10.4): the final group stays sealed and
every candidate and its spend is recorded. The final list never enters the
proposal or fitting step; `refine.py` refuses to read a proposals file that
names a final task.

### 9.7 Migration of the affordance strings

The nine producers in `affordance.rs:9-84` and `todo/text.rs:204` become
edges with `condition: always` (or the state they already branch on),
guidance equal to today's string; `affordance.rs` keeps `NEXT`, `append` and
`call_template` (the one derived from a schema, :57) and gains `render(graph,
last_call, facts)`. `crates/runtime/tests/affordance.rs` pins that every
string rendered today renders identically from the migrated graph before
any edit is accepted (`affordance::every_line_rendered_today_renders_from_the_graph`).
The `next:` contract at `affordance.rs:5-6`
("deterministic, at most two lines, immutable once written") is kept
verbatim: rendering is a pure function of the graph version and the facts.

### 9.8 The efficiency gate on tokens

`axes.py`'s `input + cacheRead + output` per solved task (`evals/axes.py:128-131`),
never turns or steps; the paper's own caveat (guidance cost 33-55 percent
more tokens on some tasks) is why the gate is on tokens, not on calls.

## 10. The sweep and the measurement

### 10.1 The levers manifest (`evals/levers/levers.json`, every current constant)

The manifest is an inventory: it names every constant, its home and its
range so a sweep can be reproduced and a default change can be priced. It
is not a search space: §10.4 lets at most three to five of these move per
campaign, and correctness constants (caps that are fuses, coverage floors,
authority rules) are listed for the record and marked `tunable: false`.

| lever | module:line | default | range | kind |
|---|---|---|---|---|
| `plan.width_max` | `ops.rs:316` (the clamp's upper bound) | 8 | 1..16 | admission |
| `plan.spawn_cap` | `ids.rs:340` | 64 | 8..256 | fuse |
| `plan.retry_cap` | `table.rs:186` | 8 | 1..16 | fuse |
| `plan.stale_turns` | `plan/mod.rs:49` | 12 | 4..40 | nudge |
| `plan.probe_first_s`, `plan.probe_max_s` | `probe.rs:14,17` | 60, 1800 | 10..600, 300..7200 | timer |
| `plan.nudge_cap`, `plan.stop_cap` | `loop_coupling.rs:18-19` | 2, 2 | 0..6 | loop |
| `plan.multi_step_score`, `plan.long_prompt_words`, `plan.enumerated_min` | `loop_coupling.rs:20-22` | 2, 30, 2 | 1..6, 10..120, 1..6 | gate |
| `plan.done_refusal_cap` | §6.3 | 3 | 1..8 | envelope bound |
| `plan.judge_cap`, `plan.jury` | §6.4 | 3, 1 | 1..6, 1..5 | envelope bound |
| `plan.scatter_rounds` | §8.6 | 3 | 1..6 | shape |
| `todo.nudge_work`, `todo.artifact_steer_turn`, `todo.artifact_cap`, `todo.quiet_turns`, `todo.first_list_work`, `todo.nudge_cap`, `todo.intercept_cap`, `todo.empty_stop_cap`, `todo.ladder_top` | `todo/coupling.rs:23-32` | 12, 3, 8, 3, 3, 2, 6, 3, 3 | per the module's own comments | loop |
| `family.max_children`, `family.cap`, `family.depth` | `subagent.rs:18,20`, `config.rs:60` | 8, 16, 1 (≤ 3) | 2..16, 4..32, 1..3 | admission |
| `family.stuck_idle_s` | `family.rs:9` | 300 | 60..1800 | timer |
| `mail.wait_min_ms`, `mail.wait_max_ms`, `mail.context_keys`, `mail.context_value`, `mail.context_total`, `mail.discoveries` | `mailbox.rs:10-20` | 1000, 300000, 8, 4096, 16384, 16 | bounded | caps |
| `lane.slots` | `lane/mod.rs:17` | 3 | 1..8 | pool |
| `route.complex_at`, `route.oneshot_at`, `route.tool_calls_per_turn`, `route.files_matched` | `ext/orchestrate.rs:88-89,134-135` | 4, −3, 4, 5 | 1..10, −6..0, 2..12, 2..20 | prefilter |
| `loop.length_stop_at`, `loop.cut_stop_at`, `loop.repeat_steer_at`, `loop.repeat_stop_at`, `loop.reasoning_cap` | `crates/loop/src/run.rs:358,361,471-472`, `reasoning.rs:5` | 3, 12, 3, 6, 48000 | bounded | loop |
| `tools.reduce_floor` | `crates/tools/src/reduce.rs:17` | 8192 | 2048..32768 | reducer |
| `advisor.cadence` | `advisor/mod.rs:17` | 25 | 5..100 | cadence |
| `review.timeout_s` | `auto_review.rs:15` | 30 | 10..120 | envelope |
| `graph.next_lines` | `todo/text.rs:6` | 3 | 1..5 | render |

### 10.2 One gate module, a fixture on both sides

`crates/runtime/src/levers.rs` (new, ≤ 300 lines): `pub struct Levers` with
one field per row above, `Default` equal to today's values, consumed by
each module through `RuntimeWiring.levers` (the modules keep their `const`
names as the defaults; one call site per constant reads the field).
`Levers::from_env()` reads `YI_LEVERS=<path>` only when the process is in an
explicit eval mode (the adapter's flag the harness already sets, never a
production default); unset, or set outside eval mode, means `Default` and
the file is not opened. Fixture on both sides: `evals/levers/default.json` must equal
`serde_json::to_value(Levers::default())` (`crates/runtime/tests/levers.rs`),
and `evals/levers.py --selfcheck` (run by `evals/selftest.py`) must find
every name in `levers.json` in `default.json`. A default that moves is a
changelog row naming the ledger row that moved it.

### 10.3 Correctness, activation, value: three gates, not one

| gate | establishes | evidence |
|---|---|---|
| correctness | the supported paths obey the invariants | the deterministic adversarial, transition, concurrency and crash tests of §11 |
| activation | the mechanism ran | host-origin records (§10.6) linked to real attempts: a `spawn_intent` with its result, a `verification_requested` with a refusal then a pass on the same todo under unchanged criteria |
| value | outcomes improve at an acceptable cost | paired baseline and candidate runs, independently graded (the task's own verifier, never the self-authored contract alone), complete cost accounting including failed attempts, verifiers and retries |

Rows 0022 and 0024 (`docs/eval-ledger.md:60,62`) both read 21/21 while cost
went $0.0524 → $0.1256 and wall 907.8 → 2724.2 s across several changes:
activation and an unchanged pass count are not value. The F0e value gate:
existing fixture correctness preserved; independently graded benchmark
success with per-task failure analysis showing no unexplained regression; the
targeted reliability failure eliminated in its controlled journey; aggregate
fully accounted cost and median task wall within **10 percent** of the
baseline on the unchanged suite (the owner's number, 2026-09-13); when noise
prevents a conclusion, the narrowly needed extra runs; a reliability fix that
exceeds the tolerance is a documented trade-off and an explicit owner
decision before default rollout. Floors per task class
(`evals/levers/floors.json`, `{class: {pass_min, reward_min, tolerance}}`;
first cut `code` = html-js-filter, bun-sourcemap-leak, cargo-flight-dispatch;
`data` = heat-pump-warranty, foodstuff-beta-activity; `science` =
photonic-waveguide-routing; `fixtures` at 1.0) and SoL-Pi's two gates for any
lever change: (1) every capability metric within `tolerance` of its floor;
(2) at least one efficiency metric (`costUsd`, `tokens`, `wallSec`) improved;
survivors nondominated. A candidate that improves cost by failing a floor is
rejected with the class named
(`test_levers.py::a_cheaper_candidate_below_the_floor_is_rejected_with_its_class`).

### 10.4 Task groups, the protocol manifest, tuning policy

Three disjoint groups in `evals/levers/split.json`: development (failures
inform proposals; fitting uses it), validation (candidates are selected on
it; every access is recorded as selection), final (the frozen candidate runs
once; a final that informed a change retires into validation and a new final
is drawn). With thirteen tasks today the final group is empty; it is filled
from the `fan-out` task and every task added after it, and related variants
stay in one group. A run commits a protocol manifest first: baseline and
candidate revisions, model ids and routing, effort, budgets, task checksums,
repetitions, run order, environment fingerprint, graders, exclusions and the
decision rule; paired seeds where the provider supports them, else paired
runs with interleaved order.

Tuning: at most three to five levers tied to observed failures (first
candidates: `plan.done_refusal_cap`, `family.stuck_idle_s`, `plan.width_max`,
`todo.nudge_work`), each with a validated range and integer or categorical
type; controlled comparisons or a bounded grid over those, every run counted
in the search spend. No ordinary-least-squares fit over the manifest (48
knobs need 49 parameters before task effects; 40 rows cannot identify them)
and no handwritten Gaussian process; a later optimizer needs enough
independent configurations, a justified model and a reviewed implementation.
Correctness invariants, authority rules and minimum coverage are never
tunable.

### 10.5 Token cost, not steps

Cost is `input + cacheRead + output` and `costUsd` from `axes.py`
(:128-133); turns and tool calls are reported, never optimized (Procedural
Graphs cut tool calls 81.8 percent while raising tokens on some tasks; §15).

### 10.6 Which host records prove each stage

Signals derive from versioned host records, not from cell text; the
text-derived `children_spawned` (`extract.py:389-397`) stays as a labelled
legacy metric and is never summed with the record-derived one.

| stage | record and derived signal (each with a `fixtures/signals.jsonl` entry) |
|---|---|
| F0a | `plan.op` request records; `wait` cursor records; lifecycle deliveries with their wake state |
| F0b | journal transactions, request deduplication hits, `spawn_intent`/`spawn_result` pairs (`plan_children` = intents with results, deduplicated by effect id), reconciliations |
| F0c | `verification_requested`, verdicts by outcome, `done_refused`, `done_after_refusal` (same todo, linked attempts, unchanged criteria digest), `verification_stale` |
| F0d | candidate, integration and acceptance records; dispositions |
| F1 | source records linked to plan requests (`program_cells`), scheduler lease and shape records (`shape_runs`), `spec_drift` |
| F2 | envelopes by kind, receipts by state, `revocations`, `repossessions`, cancel latency (due vs observed), `requests_timed_out` |
| F3 | juror votes by outcome (`juror_pass`, `juror_fail`, `juror_abstain`, from the record's `jurors` counts), quorum outcomes and escalations (the `verdict_*` signals), `quotes_dropped` |
| F4 | tokens per solved task against the baseline; `graph_version` in the session header; `levers_hash` in the config fingerprint |

### 10.7 The F0e run

Default suites: the seven fixture tasks at k=3 per condition
(`evals/run.py`, about five cents), the six-task slice at k=3 per condition
(`TBV4_ATTEMPTS=3 sh evals/drivers/tbv4_baseline.sh`, `evals/drivers/tbv4_baseline.sh:9-15`;
row 0025 spent $0.77 at k=3, `docs/eval-ledger.md:63`), and the mechanism
journeys: two independent deliverables (the `fan-out` task in harbor layout
under `evals/fixtures/tasks/fan-out/`), a failing checker then a product
repair under unchanged criteria, a controlled kernel restart, an integration
conflict, and a simple request that must not spawn. Fault injection lives in
the non-paid tests; the live journeys confirm the interfaces (the faux
provider cannot script a kernel, `2026-09-08-pass-levers.md:684-686`, so the
activation evidence is paid by construction). Scored by `evals/axes.py` plus
the record-derived signals; one ledger row with baseline and candidate
fingerprints, all costs (input, cache read, cache write, output, settled
charges or a labelled estimate), wall, recovery outcomes, and the §10.3
decision. If the run shows activation without value, the next move is the
mechanism or the evaluation, not F1; an independently proven fix (F0a) may
land on its own correctness evidence without claiming the platform passed.
Row 0025's own numbers are read honestly: 3/18 is one task passing three
times (`html-js-filter` 3/3, :63), and a signal that misses engine spawns
proves nothing about them.

## 11. Stages F0 to F4

Conventions for every stage: forge issue first (milestone in the header),
fixtures before source, `just check` green, `Ratchet:` commits for
`test_size_budget.json` and `src_loc.json` in their own commits, one
changelog row with a `growth +N:` memo, `just adr`, `pr open`, `pr merge`,
the next PR of the stack opens after its predecessor merges. Tiers per D85:
T0 unit, T1 faux, T2 real-binary journey (`just journeys` marker exact),
T3 paid. Kernel-dead path named per stage. File placement respects the
1,200-line cap (§2 B21): new behaviour goes in new files where the host file
is within 150 lines of the cap.

### F0a · `plan.op` is one request over the engine, and a child's lifecycle wakes its parent (D-next-1; extends D137, D165)

**Scope.** One host request sharing the tool's parser and engine; the
dangling plan skill deleted in the same PR; race-safe lifecycle delivery
through `wake_idle_hook`; `wait` with a per-caller cursor and states; the
`RLMSpawnHandle.result` poll replaced; the old `updated` reply kept one
release. Storage format unchanged in this PR.

**Files.** `crates/runtime/src/plan/mod.rs:377-392` (register `plan.op`
beside `plan.get`), `plan/tool.rs:344` (the parser is shared), `wiring.rs:603`
(the child host's `notice`), `session/hooks.rs:82-96` (`wake_idle_hook`
reused), `mailbox.rs:286-319` (`wait` cursors; a new `mailbox/cursor.rs` if
the file nears the cap), `subagent.rs:715,719` (notice, `states()`),
`python/yi_runtime/src/rlm/__init__.py:52-83,390` (`plan_op`, cursor
`wait`), `python/skills/plan/` (deleted) and `crates/kernel/src/bootstrap.rs:274`,
`crates/runtime/tests/ext_e2e.rs:350` (`plan.` names if a prompt names them).

**Signatures.** `registry.register("plan.op", |payload| …)` with payload
`{request_id, plan?, expected_revision?, op, args}`; actor fixed at
registration (`Actor::Owner` on the root kernel's registry, `Actor::Child(name)`
on a child's, the way the child link overwrites `agent_message.send`,
`wiring.rs:508`); the handler runs `engine.apply` on `spawn_blocking`
(`plan/mod.rs:383`); `expected_revision` is compared for non-commutative ops
(request deduplication lands with the journal in F0b; until then a duplicate
`request_id` within one process returns the cached reply). `rlm.wait(timeout,
cursor)` returns `{cursor, changed, states, notes, updated}`.

**Fixtures (before source).** The three walkthrough fixtures replayed as
`plan.op` payloads (a second driver in `plan_walkthrough.rs`); a two-child
session fixture for the wake tests.

| control | test (tier) |
|---|---|
| one parser, one engine, one actor rule for both surfaces | `plan_walkthrough::every_fixture_replays_identically_through_plan_op` (T1); `plan_tool::the_tool_and_the_request_refuse_an_actor_argument` (T0) |
| a child kernel may only view its parent's plan | `plan_e2e::a_child_kernels_plan_op_is_refused_beyond_view` (T1) |
| a finished child wakes an idle parent | `plan_dispatch::a_childs_finish_wakes_an_idle_owner` beside :537 (T1) |
| a notice arriving at the parent's `AgentEnd` is not lost | `recursion_e2e::notice_arriving_at_parent_turn_end_is_not_lost` (T1, a controlled interleaving) |
| two children finishing together coalesce wakes and lose no update | `recursion_e2e::concurrent_notices_preserve_updates_and_coalesce_wakes` (T1) |
| two waiters observe their own completions | `recursion_e2e::two_waiters_observe_their_own_child_completion` (T1) |
| a late waiter sees the terminal state | `recursion_e2e::late_wait_observes_already_completed_child` (T1) |
| the Python handle awaits `wait`, keeps its timeout and missing-child errors | `python/yi_runtime/tests/test_rlm.py::result_uses_wait_and_keeps_its_errors` (T0) |
| the skill's replacement is installed and usable | `kernel_data_surface::the_plan_skill_is_gone_and_plan_op_answers` (T2) |
| restore and store notices still do not start a turn | `ext_e2e::effects_apply_in_emit_order_and_reminders_reach_the_notice_hook` (T1, existing, :374) |

**Prompt bytes.** `doctrine.md:347-357` rule 5 gains "and its `states`":
about +20 bytes. **LOC.** yi-runtime +180 −20, python +40 −110 (the skill).
Net about +90; no memo expected, measured at land. **Issue.** "F0a plan.op
and lifecycle wake". **Row.** "Plan ops are one host request over the engine
the tool uses; a child's lifecycle notice wakes its parent through the
idle-waiting hook; `rlm.wait` takes a cursor and returns states; the
kernel-side plan skill that called four unregistered names is deleted
(D-next-1, extends D137 and D165; Closes #<n>)". **ADR.** "D-next-1: plan
ops are a host request; lifecycle notices wake; wait is cursored". **Exit.**
`just check` and the T1 journeys green; `yi ask --here --yolo` with a cell
that spawns one child and ends the turn blocked on it starts the next turn
by itself when the child finishes; the JSON tool unchanged. **Kernel-dead
path.** untouched. **Rollback.** unregister one name and restore the one
line at `wiring.rs:603`; persisted deliveries are never discarded by the
revert.

### F0b · The plan store is a journal with a typed checkpoint; recovery reconstructs; authority is a channel (D-next-2; amends D97, D105; carries D26, D53)

**Design gate before source.** The record schema of §5.3, the commit
protocol, the crash matrix, the request and effect identities, the import
mapping and the authority path are written as fixtures and a reducer test
first; the storage ADR names the platform assumptions.

**Scope.** §5 entire: `plan.json` format 2 with schema validation and a
domain validator; `ops.jsonl` as the journal (commit point, digest chain,
checkpoint after); the pure reducer and `repair` as reconstruction then
reconciliation; `spawn_intent`/`spawn_result` around `delegate.spawn`;
request deduplication and `expected_revision`; `Op::Import` (explicit,
lossless, artifact blob) and the format-1 reader kept two releases;
deletion of hand-edit folding; `Actor::User` minted only by the confirmed
path; `yi plan fuse reset` and `yi plan repair` with confirmation; the
`.gitignore` line; admission as a predicate; artifact blobs under the plan
directory.

**Files.** `crates/types/src/plan/{doc.rs, ids.rs, ledger.rs}`,
`crates/types/src/plan/plan.schema.json` (new), `crates/runtime/src/plan/{store.rs, ops.rs, table.rs, ledger.rs, mod.rs}`,
new `plan/journal.rs` (append, digest, flush, read, tail repair), `plan/state.rs`
(the reducer), `plan/recovery.rs`, `plan/import.rs`, `plan/artifact.rs`
(blobs by digest); `crates/cli/src/plan.rs`, `crates/cli/src/rpc.rs:217-230`
(the confirmed administrative actions), `wiring.rs:456`, `gate.rs` (the
confirmation binding), `scripts/guardrails/baselines/schemas.lock`
(`--update`, own commit).

**Types.** `PLAN_FORMAT = 2`; `PlanRepr` gains `intent`, `constraints`,
`examples`, `shape`, `placement` (null), `journal_seq`, `journal_digest`;
`TodoRepr` gains `note`, `attempt`, `refusals`, `contract_hash`,
`resolution: Option<Resolution>`; `JournalRecord { #[serde(flatten)] record:
PlanOpRecord, seq, request_id, expected_revision, attempt, args, args_hash,
program_hash, verdict, digest }`; `Op::FuseReset`, `Op::Repair`,
`Op::Import`, `Op::Resolve { attempt, resolution }`, `Op::Accept`;
`PlanEngine::new(store, journal, telemetry: Arc<dyn OpSink>, delegate)`;
`fn reduce(records: &[JournalRecord]) -> Result<RootState, ReduceError>`
(no effects, by construction: it takes no delegate); `fn admit(plan, label,
slots) -> Result<(), Refusal>` replacing `admissible`; canonical JSON in
`yi-types` with golden vectors shared with `python/yi_runtime/tests/vectors/`.

**Fixtures (before source).** `fixtures/plans/format1/campaign.md` and the
expected `format2/campaign/plan.json` plus artifact digest; a 6 KiB
body-section file; `format2/corrupt/plan.json`; a journal with a torn last
line; a journal with a damaged middle record; a checkpoint whose `seq` is
behind its journal; a schema-valid edited `plan.json`; canonical JSON
vectors; an agent-invoked CLI transcript.

| control | test (tier) |
|---|---|
| a journal failure never acknowledges success | `plan_journal::journal_failure_never_acknowledges_uncommitted_success` (T0, injected write and flush failures) |
| a crash after commit, before the reply, returns the same result on retry | `plan_journal::crash_after_commit_before_reply_returns_same_request_result` (T0) |
| one transaction covers a root and its sub-plan | `plan_journal::root_transaction_covers_parent_and_subplan_changes` (T0, crash between projected writes) |
| recovery executes nothing | `plan_recovery::recovery_reduces_events_without_dispatching_effects` (T0, a delegate that panics on any call) |
| an ambiguous spawn is reconciled, never reissued | `plan_recovery::ambiguous_spawn_is_reconciled_not_reissued` (T0, crash after `spawn_intent`) |
| a damaged committed record stops recovery | `plan_recovery::corrupt_middle_record_blocks_recovery` (T0) |
| a torn tail is set aside with its bytes kept | `plan_recovery::incomplete_tail_is_preserved_and_repaired_by_policy` (T0) |
| an edited checkpoint cannot overwrite the journal | `plan_store::edited_export_cannot_overwrite_authoritative_state` (T0) |
| the schema and the domain validator refuse with the path | `plan_store::a_corrupted_plan_json_is_refused_with_the_failing_path` (T0) |
| an agent's CLI cannot mint user authority | `plan_ops::agent_cli_cannot_reset_fuse_as_user` (T1) |
| a confirmed user op carries its citation | `plan_ops::a_user_op_is_recorded_with_its_user_citation` (T1, console rpc) |
| the fuse only resets by the confirmed op | `plan_ops::fuse_reset_is_the_only_writer_that_lowers_spawns` (T0) |
| import keeps every byte and every note | `plan_import::import_preserves_all_markdown_and_large_notes` (T0) |
| a skipped release still imports | `plan_import::skipped_release_can_still_import_format_one` (T0) |
| import is explicit, a read stays read-only | `plan_store::a_format_1_read_is_read_only_until_import` (T0) |
| canonical hashes agree across Rust and Python | `plan_journal::canonical_hashes_match_across_rust_and_python` (T0) and the Python twin |
| hand edits are not folded | `plan_store::a_file_changed_behind_the_engine_is_detected_not_diffed_in` (T0) |
| admission refuses, never orders | `plan_table::admit_refuses_the_ninth_delegated_start_with_the_count` (T0) |
| the plan never requires the kernel | `plan_walkthrough::every_fixture_completes_through_the_json_tool_with_no_program` (T1) |
| the fuzz lane holds | `plan_fuzz` gains `FuseReset`, `Repair`, `Import`, a user actor and crash points (T0) |
| the journal is durable before the checkpoint | `plan_journal::a_failed_sync_acknowledges_nothing_and_the_checkpoint_is_untouched` (T0, an injected `sync_data` failure); `plan_journal::kill_nine_between_append_and_checkpoint_recovers_the_record` (T1, a real subprocess) |
| an over-long journal record is damage, not a shorter record | `plan_journal::an_unterminated_record_past_the_cap_is_refused_and_resynchronized` (T0) |
| a socket peer cannot confirm an administrative op | `rpc::a_socket_client_cannot_confirm_an_administrative_op` (T1; pinned once the daemon socket's trust is verified) |
| mutation adequacy on the modules that matter | `cargo mutants` on `contract.rs`, `journal.rs`, `state.rs`, `recovery.rs` in the nightly lane; every survivor triaged in the PR |

**Prompt and doc changes.** `plan` tool DESCRIPTION (`tool.rs:569`): the
sentence about the file goes; about −60 bytes. `docs/YI_DESIGN.md:1211-1246`
rewritten (§12). **LOC.** yi-types +200, yi-runtime +620 −180 (journal,
reducer, recovery, import, artifacts; minus `fold_user_edits`, `user_edits`,
frontmatter writer), yi-cli +160. Net about +800; memo: `growth +800: the
journal, its reducer and recovery, explicit import and the confirmed user
path; the Markdown writer and the hand-edit fold were deleted for them`.
**Issue.** "F0b the plan store is a journal with a typed checkpoint".
**Row.** "The plan store is an append-only journal with a typed JSON
checkpoint and artifact blobs; recovery reconstructs without effects and
reconciles the rest; hand edits are gone and a user's administrative ops are
confirmed in the session (D-next-2, amends D97 and D105; Closes #<n>)".
**ADR.** "D-next-2: the plan journal, its checkpoint, and user authority as a
confirmed channel". **Exit.** the crash matrix green on macOS and the Linux
CI runner; the three walkthrough fixtures complete through the new store
without a kernel; `yi plan repair` on a hand-corrupted `plan.json` rebuilds
it and lists any Running todo as `NeedsReconciliation`. **Rollback.** the
original files are untouched by import; a compatible binary reads format 1;
downgrading an executed format-2 root needs the export path, not a
`git checkout` of views.

### F0c · done is verified on every completion path (D-next-3; retires D77's fields)

**Scope.** §6.1-6.5 without the judge (`judge` items refused at
declaration); the contract on the todo; attempts; the verification token;
`done_refused`, `verification_stale`, the refusal cap as an explicit
transition; `set` through the validator; walls on `SpawnSpec` passed by
`kwargs_of`; the D77 fields removed; the examples runner. Worktree acceptance
is F0d: until it lands, `done` on a worktree todo is refused as unavailable.

**Files.** `crates/types/src/plan/contract.rs` (new), `doc.rs`
(`Todo.contract`, `attempt`, `SpawnSpec.wall`), `crates/types/src/plan.rs:60-71`
(fields removed; values land in `extra`), `crates/runtime/src/plan/verify.rs`
(new: the verifier, ≤ 400 lines; `validate_product` :810-838 hoisted),
`plan/ops.rs` (`do_done` and `do_set` call the validator), `plan/dispatch.rs:84-107`
(walls), `goal/mod.rs:46` (`run_check_in(cwd, …)`), `schema.rs` (the
assertion-keyword refusal), `skills/yi/session-mining/extract.py`,
`crates/types/tests/wire_roundtrip.rs` (the readmit fixture parses into `extra`).

**Signatures.**

```rust
pub struct Verifier { timeout_ms: u64, judge: Option<Arc<dyn Judge>> }                       // F3a wires the jury in
impl Verifier { pub fn run(&self, token: &VerificationToken, contract: &Contract, snapshot: &Snapshot) -> Verdict }
pub enum PlanOpError { … Refused { label: TodoLabel, verdict: Verdict }, Stale { label: TodoLabel, token: VerificationToken },
    ContractDrift { label: TodoLabel }, AcceptanceUnavailable { label: TodoLabel } }
```

**Fixtures.** `fixtures/plans/contracts/`: `writer-cmd-red-then-green.json`
(a `done` refused once on a failing checker fixture, then passing after the
product, not the checker, is fixed), `set-cannot-complete.json`,
`reader-schema.json`, `examples-runner.json` with `cases.json`,
`legacy-stated-unverified.json`, `unserved-output-today-passes.json` (pins
the current implicit pass at `ops.rs:836-838` as red first), `stale-token.json`.

| control | test (tier) |
|---|---|
| `set` cannot complete a failing todo | `plan_ops::set_cannot_complete_a_failing_task` (T0) |
| every surface needs a matching verified completion | `plan_ops::every_surface_requires_matching_verified_completion` (T1: tool, `plan.op`, CLI, import, repair, restored view) |
| a critical abstention never aggregates to pass | `contract::critical_abstention_never_aggregates_to_pass` (T0) |
| the writer floor needs a passing critical behavioural check | `plan_ops::writer_requires_a_passing_critical_behavioral_check` (T0) |
| an unserved output or schema never passes | `plan_ops::missing_output_schema_or_resolver_never_passes` (T0; red against today's :836-838) |
| an unsupported schema assertion is refused | `schema::unsupported_schema_assertion_is_refused` (T0) |
| an old attempt's verdict cannot complete a restarted todo | `plan_ops::old_attempt_verdict_cannot_complete_restarted_task` (T0) |
| a changed criterion or output invalidates the verdict | `plan_ops::changed_criterion_or_output_invalidates_verdict` (T0) |
| concurrent `done` calls share one verification | `plan_ops::concurrent_done_requests_share_verification_effect` (T0) |
| a checker gains no permission through its contract | `verify::checker_cannot_gain_permission_through_contract` (T0) |
| a product repair passes under unchanged criteria | `plan_ops::product_repair_passes_under_unchanged_criteria` (T1) |
| JSON equality ignores whitespace and key order | `verify::example_json_equality_ignores_whitespace_and_key_order` (T0) |
| an inline todo's output is the product, not a sidecar | `plan_ops::inline_task_output_validates_product_not_sidecar` (T0) |
| the refusal cap is an explicit transition | `plan_ops::the_third_refusal_blocks_the_todo_on_user_as_a_recorded_transition` (T0) |
| plan-dispatched readers are walled | `plan_dispatch::kwargs_carry_the_wall` (T0) |
| D77's fields are gone and old records still parse | `wire_roundtrip::readmit_lands_in_extra` (T0) |
| the fuzz lane holds with contracts | `plan_fuzz` asserts no `Done { VerifiedDone }` without a committed `Pass` verdict (T0) |
| `aggregate` never panics and is monotone | `contract::aggregate_never_panics_for_any_item_set` and `contract::adding_a_critical_fail_never_raises_the_outcome` (T0, proptest); a `kani` harness over 16 items in its own pinned lane, optional |
| the checker sees no provider key | `verify::the_checker_sees_no_provider_key` (T0; a checker that prints its environment) |
| a checker's grandchild dies at the deadline | `verify::a_checkers_grandchild_is_killed_at_the_deadline` (T1; `sh -c 'sleep 600 &'`) |
| the `plan.op` parser refuses unknown keys | `plan_tool::an_unknown_argument_key_is_refused` (T0; verify `parse_op`, `tool.rs:344-350`, before writing it) |

**Prompt bytes.** `doctrine.md:343-346` rule 4 loses "you run the check"
and gains "done runs the contract; a refusal names the item": about +40
bytes. **LOC.** yi-types +260, yi-runtime +480 −60, python (extract) +40.
Net about +720; memo: `growth +720: the contract type, the verifier, the
token and the closure of every completion path; D77's unread ladder fields
were deleted`. **Issue.** "F0c done is verified on every path". **Row.**
"done runs the todo's contract in the kernel against a frozen attempt and
refuses with a recorded verdict; set, import, repair and supersede go through
the same validator; a critical abstention cannot pass (D-next-3, retires
D77's fields; Closes #<n>)". **ADR.** "D-next-3: completion is verified by
the kernel on every path". **Exit.** the red-then-green fixture is red then
green; `unserved-output-today-passes` is red before and refused after; a
generated state-machine trace shows no `VerifiedDone` without a `Pass`.
**Rollback.** `Todo.contract` reads back as `accept` through the import
path for one release; verified records are never downgraded to the
unchecked path while claiming equivalence.

### F0d · Worktree results are accepted, and failures are cleaned up without merging (D-next-4)

**Scope.** §6.6: candidate submission, candidate and integration checks,
serialized publication with a generation check, explicit dispositions,
quiescence before settle, retained artifacts; `reap` records a disposition
instead of dropping the lane; the stuck notice and its due time on the
probe loop (§7.5; the `Notify` lands here so F2b's revoke reuses it); no
tournament.

**Files.** `lane/mod.rs:939-955,1013-1052` (`Lane::settle()`, dispositions),
new `plan/acceptance.rs` (the integration coordinator; `lane/mod.rs` is at
1,053 lines), `plan/dispatch.rs` (`worktree_state`, `settle`), `mailbox.rs:491`
(`reap` disposition), `subagent.rs:851-858` (`delete` consults the
disposition), `probe.rs:105,233-260` (the stuck job, the `Notify`),
`family.rs` (a `StuckLatch` keyed by child name), artifact retention.

| control | test (tier) |
|---|---|
| a failing candidate never touches the parent checkout | `lanes::failing_candidate_never_contaminates_parent_checkout` (T1) |
| a moved parent generation rejects a stale integration | `lanes::changed_parent_generation_rejects_stale_integration` (T1) |
| a discarded worktree is not reported merged | `lanes::discarded_worktree_is_not_reported_as_merged` (T1) |
| a failed unmerged todo can retain or discard and finish | `plan_ops::failed_unmerged_task_can_preserve_or_discard_and_finish` (T1) |
| a merge conflict keeps a recoverable candidate | `lanes::merge_conflict_retains_recoverable_candidate` (T1) |
| cleanup preserves artifacts before releasing the slot | `lanes::cleanup_preserves_artifacts_before_releasing_slot` (T1) |
| a writer is quiescent before snapshot or settle | `lanes::writer_is_quiescent_before_snapshot_or_settle` (T1, a background command) |
| the user's dirty tree survives an integration | `lanes::user_dirty_tree_is_preserved_during_integration` (T1) |
| full worker capacity cannot deadlock verification | `lanes::full_worker_capacity_does_not_deadlock_verification` (T1); `plan_ops::full_worker_capacity_does_not_deadlock_verification` (T0, the counters at the product constants) |
| `done` on an unmerged worktree todo is refused, `fail` is not | `plan_ops::a_worktree_child_cannot_be_marked_done_before_acceptance` (T0, over the stub delegate; the lanes bench covers the T1 side) |
| one stuck notice per episode, and an earlier due time interrupts the sleep | `family::a_stuck_child_is_reported_once_until_its_records_move` (T0, clock injected); `plan_probe::an_earlier_due_time_wakes_the_loop` (T0) |
| a lane that cannot settle stays held, never dropped onto the next claim | `lanes::a_lane_that_cannot_settle_stays_held` (T1, a background command) |
| the user's untracked directory survives a ref-only publication | `lanes::an_untracked_directory_survives_a_ref_only_publication` (T1) |
| a publication runs from the repository root whatever the session cwd | `lanes::a_publication_runs_from_the_repository_root_whatever_the_cwd` (T1) |
| a fast-forward failure that is not the user's dirt publishes nothing | `lanes::a_transient_fast_forward_failure_publishes_nothing` (T1) |
| a stale integration left unprepared is prepared by the next `done` | `lanes::a_stale_integration_left_unprepared_is_prepared_by_the_next_done` (T1) |
| a second `submit` of one token replays its settled refusal | `lanes::failing_candidate_never_contaminates_parent_checkout` (T1) |

**LOC.** yi-runtime +420. Memo: `growth +420: candidate acceptance,
integration, dispositions and the stuck notice`. **Issue.** "F0d worktree
acceptance and cleanup". **Row.** "A worktree todo is done only when its
candidate and its integration both pass and the acceptance is recorded;
every other exit records a disposition and never merges to free a slot; a
stuck child is reported once per episode (D-next-4; Closes #<n>)". **ADR.**
"D-next-4: candidate, integration, acceptance, disposition". **Exit.** the
writer path and the conflict path complete through the JSON tool with
durable evidence and no kernel. **Rollback.** candidates and unresolved
integration records are retained; never a merge.

### F0e · Measure and decide (no D-row)

The §10.6 record-derived signals with `fixtures/signals.jsonl` entries and a
regenerated `evals/fixtures/axes/expected.jsonl`; the `fan-out` task; the
protocol manifest; the §10.7 runs; one ledger row with the §10.3 decision
(correctness, activation, value at the owner's 10 percent). **Exit gate for
F1-F4:** the decision recorded. Activation without value sends the work back
to the mechanism or the evaluation; F0a and any independently proven fix may
land on their own evidence.

### F1a · The `yi` library: plans as programs, explicit resume (D211, landed 0.270.0; extends D166)

**Files.** `python/yi_runtime/src/yi/{__init__,plan,contract,roles}.py`,
`pyproject.toml`, `python/yi_runtime/tests/test_yi_{plan,help,idempotent,resume}.py`
and `tests/fake_host.py`, `crates/types/src/plan/op.rs` (`Op::Program { cell_id,
source_ref }`), `crates/runtime/src/plan/program.rs` (the record and the export),
`crates/runtime/src/plan/request.rs` (the typed reply, the `artifacts` rider),
`crates/runtime/src/fetch/schemes.rs` (`plan://<id>/artifacts/<sha256>`),
`crates/runtime/src/kernel.rs` (the prelude imports `yi`),
`crates/runtime/tests/ext_e2e.rs` (`yi.`, `plan.`),
`scripts/guardrails/check_prompt_examples.py`.

| control | test (tier) |
|---|---|
| `todo` is idempotent by key and refuses drift | `test_yi_idempotent::test_a_rerun_cell_writes_nothing_and_a_changed_spec_is_refused` (T0, a fake `host_request`) |
| a retried `create` is one plan | `test_yi_plan::test_a_retried_create_request_is_the_same_plan` (T0) |
| source is recorded before the first effect and never replayed | `plan_program::program_records_source_before_the_first_effect` (T0, through `plan.op`); `test_yi_plan::test_a_cell_is_recorded_once_before_its_first_effect` (T0); `kernel_data_surface::resume_after_a_kernel_death_reuses_results_and_replays_no_cell` (T2, real kernel, a delegate counting spawns) |
| an unknown external activity stays unresolved | `test_yi_resume::test_unknown_external_activity_remains_unresolved` (T0) |
| a second `run` attaches or refuses | `test_yi_plan::test_duplicate_run_calls_attach_or_refuse` (T0) |
| a verdict that judged no product leaves the attempt alone, and a child the host cannot vouch for blocks on the user | `test_yi_plan::test_a_verdict_that_judges_no_product_leaves_the_attempt_alone` (T0) |
| only the plan owner stores artifacts | `plan_e2e::a_child_kernels_plan_op_is_refused_beyond_view` (T1, extended) |
| a child that asked you something is collected, never raised | `test_yi_plan::test_a_child_asking_you_something_is_collected_not_raised` (T0) |
| an inline output gets a valid artifact id | `kernel_data_surface::an_inline_todo_completes_with_a_host_minted_artifact` (T2) |
| every public name documents itself with a valid example | `test_yi_help::test_every_name_in_all_has_a_docstring_with_a_valid_example` (T0; sync and async alike) |
| the prompt gates know the new names | `ext_e2e::fragment_examples_name_real_kernel_apis` (T0, extended) |

**LOC.** python +1,080, yi-runtime, yi-types and yi-kernel +346, tests +890 (Rust 440, Python 450). Memo: `growth
+346: the source record, the typed `plan.op` reply and its artifacts`. **Issue.**
"F1a the yi library" (#454). **Row.** "Plans are programs: the `yi` library opens,
attaches to or resumes a plan, declares idempotent todos with contracts, runs one
scheduler under a lease, and records its cells as an audit artifact (D211,
extends D166; Closes #454)". **ADR.** "D211: plans are programs in the kernel;
source is recorded, never replayed". **Exit.** the §8.2 example runs end to end
on a real kernel with a stub delegate, and the kernel-death journey passes (one
test, `resume_after_a_kernel_death_…`; no provider is reached, because a stub
delegate spawns no child). **Kernel-dead path.** unchanged from F0.

As landed, against §8 and §5.4. The tree had three gaps the design assumed closed,
and F1a closes them at the host boundary rather than around it. (1) `plan.op`
answered with rendered text only, so the reply now carries `plan` (the typed
document with `ready` and `finished`), `notices`, and on a refusal the engine's own
`kind` and a refused `done`'s `verdict`; the three-code `code` is unchanged. (2) No
surface but a test could put a criterion into a plan's artifact store, so `plan.op`
takes `artifacts` (`{media_type, text}`, owner only, capped in count and bytes,
stored under the store's own digest before the op applies); the builders and the
source record name their blobs by sha256 computed in Python with the shared
canonical rule, and an op citing a digest the store lacks is refused. A builder's
`local://` input is frozen when the todo is declared, not at `start`. (3) No url
named a stored blob, so `plan://<id>/artifacts/<sha256>` serves one, which is the
host-minted id of an inline product and of a child's answer that a `schema` item
needs. A todo's `key` lives in its label (`key: label text`, the key alone when no
label is given): `TodoSpec` has thirty-five construction sites and no field for
it, and the label is already unique per plan; a renamed label is `SpecDrift`. The
scheduler lease is the owner kernel's (`_RUNS`), so it dies with the kernel, which
is what resume needs; a lease in the store is F2b's. `programHash` is set on the
`program` record alone (the sha256 `program.py` has once that cell is appended);
other records keep null rather than pay a file hash per op. The export is appended
after the commit, and the next `program` heals any cell it lacks before its own
record is built, so a crash between the two heals without a rewrite and the hash
still names the file the cell is appended to. `Plan.create` makes its plan
before it can record into it, so the cell's record is that plan's second. The
venv identity hashed `src/rlm` alone, so an edit to `yi` would have run a stale
wheel; it hashes `src` now (`crates/kernel/src/bootstrap.rs`). Not
built: a marker for a cell that later raised (the record stands either way), a
token budget, automatic transport retry beyond the one host error that leaves a
commit unknown, and `judge`, `shapes`, `mail`, `recipes/` (F3a, F1b, F2a).

### F1b · Two shapes: `fork_join` and `scatter` (D212, landed 0.271.0)

**Files.** `python/yi_runtime/src/yi/shapes.py`, `roles.py` (`verify_quotes`),
`plan.py` (`decompose` resolves edges among its own batch),
`python/yi_runtime/tests/programs/{typo-fix,campaign,arc-game}.py` (the
walkthrough fixtures as programs), `test_yi_shapes.py`, `tests/fake_host.py`,
`crates/runtime/tests/{plan_e2e,plan_walkthrough,kernel_data_surface}.rs`.

| control | test (tier) |
|---|---|
| geometry refused before any start | `test_yi_shapes::test_a_fork_join_with_a_shared_write_set_is_refused_with_every_problem_named`, `test_yi_shapes::test_a_scatter_with_shared_partitions_or_no_lead_is_refused` (T0) |
| over-admission refused while order is the shape's | `test_yi_shapes::test_fork_join_retries_a_refused_start_and_never_reorders_the_kernel` (T0, fake host counting refusals; the same run shows a module-level shape attaching to itself) |
| restart intensity bounds retries and respects the step table | `test_yi_shapes::test_one_for_one_stops_after_max_within_window` (T0, the window and the engine's retry refusal both); `plan_e2e::a_shape_cannot_retry_a_done_or_drop_a_running_todo` (T1); `plan_ops::a_retry_opens_a_fresh_refusal_count_so_only_retry_cap_bounds_a_scheduler` (T0, added: the section 6.3 cap is per attempt, so `RETRY_CAP` is the durable bound) |
| concurrent waiters keep their own cursors | `test_yi_shapes::test_the_scheduler_and_the_model_wait_without_stealing_updates` (T0) |
| a reader's uncited, unverifiable or out-of-partition quote is dropped at the seam | `test_yi_shapes::test_scatter_drops_an_unverifiable_quote_before_the_lead_sees_it` (T0); `kernel_data_surface::both_shapes_schedule_under_the_real_admission_and_step_table` (T2, the same two drops over a real archive) |
| abstentions are dropped and rounds are bounded | `test_yi_shapes::test_scatter_ends_at_max_rounds_without_a_commit` (T0) |
| the fixtures agree as programs and as JSON | `plan_walkthrough::a_program_and_its_json_fixture_reach_the_same_plan_json` (T2) |
| both shapes hold against the real admission, step table and verifier | `kernel_data_surface::both_shapes_schedule_under_the_real_admission_and_step_table` (T2, added) |
| the overhead against the direct path is counted | `test_yi_shapes::test_both_shapes_do_useful_work_and_the_overhead_is_counted` (T0, added) |
| scatter is measured against direct retrieval | a paired journey row on the same tasks (T3, user-run; not run in this stage) |

**LOC.** python +420, tests +250. Memo: `growth +420: two shapes and the
quote seam`. **Row.** "Two shapes ship as library schedulers under the
engine's admission and step table: fork_join over isolated writers and
readers, and scatter with readers bound to partitions and quotes verified
against the archive; four more are specified with their prerequisites
(D212; Closes #455)". **ADR.** "D212: shapes are schedulers in user
space". **Exit.** independently useful work through both shapes with the
overhead reported against the direct path.

As landed, against sections 8.5 and 8.6. Python +290, tests +772 (Rust 407,
Python 365 with the three programs), no Rust `src` line. Overhead, counted in
host requests on the fake host: fork_join over two writers is 9 against the 4
ops sent by hand (one `repair`, three views and one `wait` on top, none of them
journaled); scatter over two readers and a lead is 18 against 2 direct fetches
(seven ops, four reads, one `wait`, two `rlm.result`, four fetches). The shapes
take no arguments (`MAX_RESTARTS`, `RESTART_WINDOW` and `SCATTER_MAX_ROUNDS` are
module levers), so each is a module-level function and the lease's identity
check attaches a second `plan.run(shape=fork_join)` to the first. fork_join
refuses an inline todo as well as a writer outside a worktree, since both write
the owner's workspace. Its restart window is kernel-local; the durable bound is
the engine's `RETRY_CAP`, whose `retries_exhausted` refusal ends the restarts
and stays in `run.refusals`. The section 6.3 cap counts refused verdicts per
attempt (`done.rs` `refused_verdicts`; `todo.refusals` is a lifetime event
counter nothing reads for the cap, which is what section 6.3 step 6 used to read
as, and it says the per-attempt rule now), and the scheduler sends one `done`
per attempt, so a retrying shape never reaches it; a todo the engine did block
is never retried, because only `failed` is. The scatter lead is the plan's one
inline todo, `async def lead(answers, number)` returning `{"commit": answer}` or
`{"ask": question}` (there is no `lead` object to call `commit` on), and its
product is `{"answer", "rounds"}` because a bare string is stored as text and a
`schema` item finds no JSON in it. A later round declares `<reader>-r<n>` todos
with the reader's delegation and contract and the question as the note, and the
key alone as the label, because a `TodoLabel` is eighty characters and no
newline while the lead writes the question (the review found the round-two
declaration refused on any question a real lead would ask, and the fake host now
holds the label rule); a failed reader is retried and dropped, the two legal
steps from `failed` to `abandoned`, so the plan can still finish, and the T2
journey settles one that way on the real step table. `verify_quotes` fetches
each cited url once and reads the line from it: the host's line fragment needs
the tag (`#L<a>-<b>@<tag>`) a reader does not have, so a large page is read
whole until F1c's paged `fetch` lands. It also takes the reader's partition and
drops a quote citing anything else unread: the wall that bound the reader is
cooperative (section 7.6) and the owner fetches with the owner's own reach, so a
reader could otherwise answer for a partition it was never given, or for a page
outside the archive, and the disjointness geometry checks before the first start
would mean nothing afterwards. `shapes.ANSWER` types only `quotes`, because the
host's schema subset has no union type for a nullable `answer`. A program
reaches its fixture's JSON on the projection the two surfaces share: every
plan's state and version, its todos in order (a program's key is the fixture's
label in lower case), their states, edges, attempts, retries, whether they are
delegated and what a done one output, and the committed transitions in journal
order. Not compared: `refusals` and `touched`, since the fixture's refused steps
are its own; the text of a cause or a block note; a delegation's or a contract's
contents; and the campaign fixture's `reorder` and `add_edge`, which the library
has no surface for and whose generation that fixture's `supersede` closes before
the comparison reads the store. Found on the way and fixed: `Todo.decompose`
looked a sibling edge up in the parent plan and raised, so its own docstring
example failed. Not built, and section 8.6 corrected to say so: `Reader` derives
no `deny_read` from the partition, and scatter does not either, because a wall
is cooperative and `deny_read` is paths where a partition is urls; the seam is
the binding. Also not built: a writer child as the scatter lead, `rest_for_one`
and `one_for_all`, and the four later shapes.

### F1c · Paged recall through `fetch` (D213, landed 0.272.0; extends D164)

`fetch` payload gains `offset` and `limit` (bytes for `local://`, entries
for `history://`, chars for `kernel://` past `VARIABLE_MAX_CHARS` 8,192 in
`kernel.rs`); a paged reply carries `next_offset` or null, and a request
naming neither key is answered key for key as before. Files: `wiring.rs` (the
`fetch` host request), `fetch/mod.rs` (`Page`, `fetch_page`, `into_reply`),
`fetch/schemes.rs` (three resolvers), `rlm/__init__.py` (`fetch`, `Page`).
Tests: `fetch_session::a_capped_read_names_the_next_offset_and_the_next_page_continues_it`
(T1) and three edge tests beside it. Row: "`fetch` pages: a capped read names
the next offset (D213, extends D164; Closes #456)".

**As landed.** LOC yi-runtime +148 against 90 (+22 of it from the review),
python +16 against 10. Decided at the edges: a zero, negative or fractional
number is refused, never clamped; a huge limit is the rest (a `kernel://` page
clamps to the cell's cap); an offset past the end is an empty last page; a byte
offset inside a UTF-8 sequence still serves text and `next_offset` always
advances. A page is refused on `history://<agent>/tail/N`, which the plan did
not foresee: the window is anchored at the end of a growing listing, so any
append slides it. Found by the review: a walk of the reading session's own
history never ended at a limit of one, because each page appended the fetch-log
row the next page then read, so a paged read of it now records in memory alone
(`FetchLog::remember`); and a page named beside D164's `object` was dropped
without a word, and is refused.
Follow-up, not built: `roles.verify_quotes` could read a cited line's
neighbourhood instead of the whole page, but the digest it pins is the whole
page's sha256 and a page's digest is not interchangeable with it in a stored
quote, so that needs a digest rule of its own.

### F1d · The working model speaks `yi` (no D-row, landed 0.273.0; amends D166's text)

`orchestrate.md` examples become `yi` programs (about the same bytes: the
fan-out and writer examples shrink; `help(yi)` named once); `doctrine.md`
rule 3 names the program; `identity.md` names `yi`. `request_budget`
`--update` in its own commit named in the row. Tests: `prompts.rs` (every
identifier defined in its example), `ext_e2e::fragment_examples_name_real_kernel_apis`.

**As landed.** The reader fan-out is a `scatter` plan and the writer example
a `fork_join` plan; a reader's question rides `Reader(note=)`, because the
host's brief carries the label, role, acceptance, context and note and a label
is eighty characters. Bytes: `orchestrate.md` 8,144 to 7,993, `doctrine.md`
21,770 to 21,765, `identity.md` unchanged at 5,628; the request budget 48,187
to 48,182. One control was added: `test_prompt_programs.py` runs every block
of a fragment that imports `yi` against the fake host to `verified_success`,
which is what "would actually run" means short of a real kernel. `prompts.rs`
reads `shapes.ANSWER` as an attribute, not a constant the block owes, and
prices a run's `budget=` against the cell ceiling (both examples say `"8m"`).

### F1e · Every child exit is published once, and clients reconcile (D210, landed 0.269.0; extends D165)

Added 2026-09-20 from the child lifecycle audit. Anchors are by symbol; the
tree wins over any line number. Root cause: a child's lifecycle lives in
three places (the host's record map, the event bus, each client's cache) with
no shared transition, and the two removal paths (`SubagentHost::delete`,
`SubagentHost::reap`) take the record away without a `publish`. `run_child`
then finds no record, takes the `reaped` early return and skips the terminal
update too. A TUI card frozen at `Running` is never committed and never
dropped; `rlm.delete_subagent` before a child's first event mints one such
card per call. This stage is a bug fix and lands on its own evidence,
independent of the F0e decision (§11 F0e, last sentence).

**Files.** `crates/runtime/src/subagent.rs` (`retire`, `run_child`, `spawn`,
`watch`), `crates/runtime/src/mailbox.rs` (`reap`), `crates/loop/src/interrupt.rs`
and `crates/runtime/src/session.rs` (the abort-before-first-poll race),
`crates/tui/src/app.rs` (`sync_children`, both forwarders,
`commit_finished_tasks`, the stop command), `crates/tui/src/cell.rs`
("starting"), `crates/console/src/app/chat.rs` (row cache),
`python/yi_runtime/src/rlm/__init__.py` (`RLMSpawnHandle.result`).

Repairs, smallest first, each its own commit inside the stage:

1. One `retire(key)` helper that both `delete` and `reap` call: remove the
   record under the lock, then send a final `ChildUpdate` built from the
   removed record with its status forced off `Running` (`Error` if it held
   one, else `Completed`). The TUI's existing `reduce_child_update` and
   `commit_finished_tasks` then clear the card; no protocol change. `reap`
   gains `delete`'s worktree refusal (through F0d's `take_settled_worktree`),
   so the two paths differ only in keeping the transcript.
2. An aborted run is not `Completed`: `run_child` maps an interrupted stop
   reason to `error: "interrupted"` and publishes the terminal update even
   when the record is already gone (built from the fields it holds). The
   parent's notice says interrupted, never finished.
3. `sync_children` reconciles: keep a task only if it is in the roster or
   already finished; a task absent from the roster and still `Running` is
   marked finished with cause "gone" and committed. This heals every silence
   below, including a dead forwarder.
4. Both TUI forwarders (`while let Ok(event) = events.recv().await`, parent
   bus and child bus) handle `RecvError::Lagged` the way `crates/acp/src/forward.rs`
   does: report the gap and keep going; only `Closed` ends the loop. The
   host's own `watch` task does the same and re-reads the record's counters
   from the session after a gap, so a missed `ToolExecutionEnd` cannot leave
   activity on `Executing`.
5. `spawn` releases the `children` lock before `child_factory` and
   `attach_runtime`: reserve the name and the slot under the lock (a
   placeholder record), build the child unlocked, then fill the record; a
   failed build removes the placeholder through `retire`.
6. The `ipython`-not-Done gate in `commit_finished_tasks` becomes an ordering
   hint: a finished card commits at once, placed after its spawning cell when
   that cell is still live. A lost cell-end event can no longer hold every
   card.
7. Abort before the first poll: `spawn_run` admission must not clear an abort
   fired after the run was requested. `reset_if_epoch` compares against the
   epoch captured when the run was requested, not the one read at admission;
   a deleted child never starts. The usage attribution loop in `run_child`
   moves after the record check so a retired child bills nothing further.
8. `RLMSpawnHandle.result` waits through the cursored `rlm.wait` (F0a), which
   returns states, so a parent blocked on a handle sees `stuck` and
   `needs_you`; the `list_subagents` poll is deleted.
9. The console row cache removes a row on a terminal update for an absent
   child and, past 32 rows, evicts the oldest finished row rather than
   dropping the new one silently.
10. "starting" is drawn only until the first roster snapshot; after it the
    card shows the host record's activity, and `answer_preview` from the
    record wins over the folded stream when they disagree (precedence:
    `ChildUpdate`, then roster, then raw stream).

As landed: `retire` publishes for every removal, so `run_child` stays silent on a
missing record (repair 2's second publish would have made two) and a removal after
the child's own exit repeats that terminal update rather than contradicting it; repair 5 reserves
the name and slot in a `building` list rather than a placeholder record, so a
failed build has nothing to retire; repair 6 holds a finished card only behind
the one cell it was born under, which keeps the pair test's ordering; repair 8's
poll was already gone at F0a, so the handle gained the `stuck` raise alone; a
roster-rule test, `tui_e2e::a_card_the_roster_stopped_listing_ends_as_gone`, was
added for repair 3. Repair 1's refusal also reaches the plan engine's own reap, so
the accepting road of `done` marks the published branch retained before it reaps
(`lanes::accept::an_accepted_worktree_tells_the_host_its_branch_is_kept`).

Not in this stage (YAGNI until F2b needs them): a new `ChildStatus` variant,
moving the environment hook's git probes off the runtime thread (measure
first; `spawn_blocking` if the lag tests show starvation), splitting
`SubagentHost`.

| control | test (tier) |
|---|---|
| every exit publishes exactly one terminal update with a machine-readable cause | `recursion_e2e::every_exit_publishes_one_terminal_update` (T1; parameterised over complete, error, interrupt, delete while running, delete after end and reap; a session deadline ends the turn after it settles, so a child out of clock leaves by the `complete` road, and `set_deadline` is crate-private; subscribes to the parent bus, never polls `host.list()`) |
| a client view equals a projection of the host's records | `subagent_fuzz::no_card_runs_without_a_record` (T0; sequences of spawn, finish, interrupt, delete, reap and injected lag, the `plan_fuzz.rs` pattern) |
| a lagged forwarder survives and surfaces the gap | `tui_e2e::a_forwarder_survives_a_capacity_four_bus` (T1) |
| delete before the first poll leaves no zombie run and bills nothing | `recursion_e2e::a_child_deleted_before_its_first_poll_never_runs` (T1) |
| a late subscriber still shows the last tool | `tui_e2e::a_card_adopted_after_the_first_tool_event_names_it` (T1) |
| the kernel delete journey leaves no live card and tells the parent why | `tui_e2e::a_kernel_cell_that_spawns_and_deletes_leaves_no_live_card` (T1, the client half: spawn and delete under a live cell, the bus alone ends the card, then render) and `recursion_e2e::a_kernel_cell_that_spawns_and_deletes_tells_why` (T2, a real kernel runs `h = await rlm.run(...); await rlm.delete_subagent(h)`; yi-tui has no road to the kernel bridge without a new dev-dependency) |
| a finished card commits while an `ipython` cell is live | `tui_e2e::a_finished_card_commits_under_a_live_cell` (T1) |
| spawn does not hold the roster lock across the build | `subagent_fuzz::states_answers_while_a_child_is_being_built` (T0, a factory that blocks on a channel) |
| a blocked handle sees stuck | `test_rlm_handle::result_surfaces_a_stuck_state_from_wait` (T0, fake host) |
| the console drops a gone row and keeps accepting past the cap | `console chat::tests::a_child_first_seen_at_its_end_gets_no_row_and_row_thirty_three_is_kept` (T0; without a protocol change the console cannot tell a retired child from a retained one, so the rule is that a child first heard of at its end gets no row) |

**LOC.** yi-runtime +140, yi-tui +90 −30, yi-console +25, python +15 −30,
tests +420. Memo: `growth +240: every child exit is published and clients
reconcile against the roster`. **Issue.** "F1e child exits are published".
**Row.** "A child's removal, abort and every other exit publish one terminal
update; the TUI and console reconcile against the roster, survive bus lag and
no longer hold finished cards behind a live cell (D-next-7b, extends D165;
Closes #<n>)". **ADR.** "D-next-7b: one exit, one terminal update". **Exit.**
the kernel delete journey renders no live card; the fuzz holds over 10,000
sequences.

### F2a · Envelopes and the durable inbox (D214, landed 0.274.0; extends D165)

**Files.** `crates/types/src/mail.rs` (new), `mailbox.rs:65-160` (route
builds an envelope; seq counters), `mailbox.rs:217-226` (deliver writes the
inbox entry first), `crates/runtime/src/mail.rs` (new: request futures,
receipts; keeps `mailbox.rs` under the cap), `fetch/schemes.rs:30-44,179`
(the `custom/<type>` segment), `python/yi_runtime/src/{rlm/__init__.py:343, yi/mail.py}`.

| control | test (tier) |
|---|---|
| per-pair order holds | `recursion_e2e::two_messages_from_one_sender_drain_in_seq_order` (T1) |
| the inbox is durable | `recursion_e2e::a_message_to_a_finished_child_is_inboxed_and_readable_by_history` (T1) |
| the body cap refuses and names the alternative | `recursion_e2e::a_body_over_sixteen_kib_is_refused_not_trimmed` (T1) |
| request resolves on its reply and times out otherwise | `recursion_e2e::request_returns_the_matching_reply_and_times_out_without_one` (T1) |
| a receipt states what the host did | `recursion_e2e::send_returns_queued_woken_or_inboxed` (T1) |
| the host names the sender, and a kind keeps its direction | `recursion_e2e::the_host_names_the_sender_and_a_kind_keeps_its_direction` (T1) |

LOC yi-types +80, yi-runtime +260, python +90. Memo: `growth +430: envelopes,
receipts, the durable inbox and request/reply`. Row: "Messages are
envelopes with kinds, receipts, per-pair order and a durable inbox; a request
awaits its reply (D-next-8, extends D165; Closes #<n>)". ADR: "D-next-8:
messages are envelopes".

Added 2026-09-20 (child lifecycle audit). `rlm.send` without `followup=True`
reaches `follow_up_message`, and an idle or finished child never drains that
queue while the receipt says "queued". The receipt is decided after the
delivery attempt: `queued` only when a live turn will drain it, else `woken`
(the idle child is started on it) or `inboxed`. Test:
`recursion_e2e::a_send_to_an_idle_child_is_woken_or_inboxed_never_queued` (T1).

As landed (0.274.0, D214). The three receipt tests filed above under `mailbox::`
live in `recursion_e2e`: a receipt of `queued` needs a child whose turn is held
open, and that faux child already exists there, so they are T1. Measured growth is
+462 Rust `src` lines (yi-types 77, yi-runtime 385) and 82 of Python. Where the
landing differs from sections 7.1 to 7.3:

- A plain `inform` starts no turn unless `followup=True`, as `rlm.send` always
  documented and section 7.2's last paragraph says; the table's "wakes: yes" for
  `inform` holds for the followup spelling. To an idle or finished child a plain
  send answers `inboxed` and also waits on the follow-up queue, so the next turn
  anyone starts presents it; mid-turn it answers `queued`, and that word is read
  under the session's status lock as the message is pushed, never from a status
  read that could race the turn's end. `request`, `reply`, `failure` and `cancel`
  wake, through the one admission that settles it.
- `from_incarnation` and `to_incarnation` are not on the envelope: no name is
  respawned before F3c, which adds them with the thing they distinguish. (F3c added them, D218.)
- `presented_at` is not a second custom record: the presented message is the
  transcript's own entry and carries the envelope, id included, in `details`, so
  an inbox entry with no such message is the inspectable, unpresented item.
- The id is `<sender>-<n>` with `n` counted per host, so it is unique across
  recipients; `seq` is the per-pair counter. Both restart with the host, as its
  children do.
- `history://self/...` names the reader's own transcript, because a child is
  never told the name its family knows it by; `yi.mail.inbox()` defaults to it.
- A message to the parent keeps the receipt word `delivered`: the report hook
  queues or starts the parent's turn and returns nothing, and it has nine
  constructors. The envelope is still inboxed first. This is the shape, not a
  deferral: section 7.3's three words describe a receipt for a child, and no
  later stage is on the hook to widen the hook's signature for a fourth.
- Left to the stages that need them: the cancel flag in the child's loop and
  `failed` from a `failure` envelope (F2b), `progress` into `status().note`,
  progress coalescing, a bound on inbox growth, reserved capacity for control
  kinds, and the list of outstanding conversations after a parent restart. One
  bound landed: a sender holds at most sixteen waiting requests.

### F2b · Leases: deadline inheritance, revoke, abort, parent close; capabilities shrink (D215, landed 0.275.0; extends D165, D210, D214)

**Files.** `crates/types/src/lease.rs` (new), `crates/runtime/src/lease.rs`
(new: the timer job on the probe tick, repossession), `subagent.rs:470-495`
(drawn at spawn; hereditary wall; hold compile), `mailbox.rs:491` (returned
at reap), `lane/mod.rs:939-955` (`Lane::settle()` hoisted), `gate.rs:120-124`
(compile), `doc.rs:165` (`parent_close`), `rlm/__init__.py` (`revoke`).

| control | test (tier) |
|---|---|
| what happens today at parent close is pinned before it changes | `recursion_e2e::children_at_parent_close_today` (T1; written first, kept as the regression test for the default) |
| a requested deadline past the parent's is refused, never clamped | `subagent::a_deadline_past_the_parents_bound_is_refused_with_both_numbers` (T0) |
| an earlier due time interrupts the sleep and a slow probe never delays a cancel | `plan_probe::a_revoke_due_time_wakes_the_loop_past_a_slow_probe` (T0, clock injected) |
| a restart during the grace resumes the revocation from the journal | `recursion_e2e::restart_during_grace_completes_the_repossession` (T1) |
| a stop or settle failure leaves a visible pending state, never a clean report | `recursion_e2e::a_failed_settle_reports_repossession_pending_with_references_kept` (T1) |
| `Abandon` is refused | `subagent::abandon_is_refused_until_a_supervisor_exists` (T0) |
| a lease is drawn, never minted | `subagent::a_spawn_asking_past_the_parents_deadline_or_tokens_is_refused_with_both_numbers` (T0) |
| a revoke reaches the child and its repossession record lands before termination | `recursion_e2e::revoke_delivers_cancel_then_repossesses_after_grace_with_the_record_first` (T1, clock injected) |
| uncommitted work survives repossession | `lanes::a_repossessed_worktree_keeps_its_work_on_its_branch` (T1) |
| a hold-shaped rule on a detached child compiles to deny; an attached one stays a question | `gate::a_detached_childs_ask_compiles_to_deny_and_an_attached_ones_stays_a_question` (T0) |
| walls only shrink | `subagent::a_child_cannot_spawn_with_a_smaller_wall_than_its_parent` (T0) |
| the lease returns at reap | `plan_ledger::reap_records_the_unspent_lease` (T0) |

LOC yi-types +90, yi-runtime +380, python +30. Memo: `growth +500: leases,
revocation with grace and a repossession record, hereditary walls, compiled
holds`. Row: "A child holds a lease drawn from its parent; revoke has a
grace and a visible repossession record; walls shrink hereditarily and holds
compile down at spawn (D-next-9; Closes #<n>)". ADR: "D-next-9: leases and
visible revocation".

Added 2026-09-20 (child lifecycle audit; builds on F1e). (a) One typed
terminal state replaces F1e's forced status: `ChildExit { Completed, Failed
{ class }, Interrupted, Reaped, Repossessed }` in `crates/types/src/subagent.rs`
(schemas.lock `--update`), with `class` a closed set (refused spawn, provider,
kernel death, red check, deadline); `ChildUpdate.status`, `MemberState` and
the terminal notice are all derived from it by one function, so the TUI and
the model cannot disagree. "Admitted, not started" becomes a `MemberState`
(`queued`) set at admission and cleared at the first poll. (b) `ChildRecord`
gets one transition function; `fold_event`, `run_child`, `deliver_to_parent`
and `take_pending` call it instead of writing fields. (c) `stuck` is read from
a typed loop signal, not from `custom_type` string matches. (d) The TUI stop
command goes through the host's `interrupt` (then `revoke`), never
`child.session.abort()`; `ChildView` stops handing out the session for
control. (e) `SubagentHost` sheds the roster and lifecycle into
`crates/runtime/src/family.rs` only as far as the file cap forces it; no new
crate. Tests: `subagent::status_state_and_notice_derive_from_one_exit` (T0);
`recursion_e2e::a_tui_stop_is_a_host_interrupt` (T1);
`family::stuck_reads_the_typed_signal` (T0). LOC yi-types +40, yi-runtime
+120 −60, yi-tui +10.

As landed (0.275.0, D215). Measured growth is +1032 Rust `src` lines (yi-types 146,
yi-runtime 881, 60 of them the review pass) and 23 of Python. Where the landing differs from sections 7.4 to 7.6
and the rows above:

- One test is renamed: the attached half of the hold test asserts that the question
  stands, so it is `..._and_an_attached_ones_stays_a_question`. Every child shares its
  family's one `PermissionBroker`, so an attached child's `Ask` already reaches the
  root's surface; a `request` envelope to the parent would be a second road to the same
  answer. `gate::compile_ask` is the detached half, and the broker's no-asker arm reads
  its refusal from it. The spawn refusal for a brief that names a walled path is not
  built. The `context` the section means is a `Delegation`'s list of URLs, which is
  structured and could be checked, but the effective wall is computed in `spawn`, which
  is handed kwargs and never the delegation, so the check needs the URLs plumbed to the
  one place that knows the parent's wall. F3a built it on the engine's spawn road: the
  delegate asks the host for the effective wall (`wall_for`, which `spawn` shares) and
  refuses a delegation whose `context` that wall denies.
- The lease journal is the parent's own transcript (`custom{lease}` entries: `revoked`,
  `repossessed`, `returned`), not `ops.jsonl`: a lease exists without a plan. Three
  tests were added beside the table: `a_cancel_ends_the_run_at_its_next_message_boundary`,
  `a_terminated_respondent_refuses_its_waiters_by_name` and
  `a_failure_from_a_child_reads_failed_in_wait` (all `recursion_e2e`, T1).
- "Record first" is the order inside `retire_as`: the run is stopped and joined, the lane
  settles, the `Repossession` is journaled, and only then is the record released and the
  terminal update published. The revocation itself is journaled before the `cancel` is
  sent, which is what a restart resumes from. After a restart the child's process is
  gone, so the resume completes the record and tells the parent; a worktree it held is
  an orphan lane the pool already knows how to reap, and the record's disposition says
  `pending` with that as its reason, because nothing settled it. The resume reads the
  journal oldest first, so a `repossessed` line closes the `revoked` line before it.
- The plan journal's `Disposition::RepossessionPending`, reserved in F0d, is still
  unwritten. A repossessed worktree child leaves the roster, so the engine reads a child
  it cannot vouch for and blocks its todo on the user; the disposition is journaled when
  that todo leaves `Running` through `fail` or `drop`, from the refs its `submit`
  recorded. Writing it from the repossession itself needs a road from the host into the
  engine's journal that no delegate has. F3a did not build it: the road it opened runs
  the other way, from the engine's verifier into the host, and a repossession fires on
  the probe loop's timer with no engine transaction open, so the write needs its own
  request into the engine and a stage that owns it.
- `RepossessionPending` is a `MemberState` (`repossession_pending`) over a record whose
  exit is still absent; the timer's job retries it on every wake, at the loop's one second
  floor while a probe is in flight and at its idle poll otherwise. One reference is not
  carried across a retry: a journal write that fails after the lane has already settled
  leaves the record without its lane, so the retry's `kept` names only the transcript. The
  branch itself is kept, and `yi lanes` still finds it as an orphan.
- `lease.deadline_ms` is optional: a root with no `--deadline` has no clock to lease.
  Tokens are reserved and accounted, and returned at reap; nothing ends a run for
  spending past its reservation yet, and a parent's own turns are not debited here.
  A root holds no token grant either, since no CLI flag sets one, so the refusal binds
  a grandchild against what its own parent drew and never a root's first child. Both
  spawn roads draw: `rlm.run` from `tokens`, the engine's dispatch from
  `SpawnSpec.budget`, which is no longer only a line in the brief.
- A child's `failure` envelope is a verdict on its work, not the end of its run. It
  files no exit while the run is live: the record carries it as a phase, `wait` reports
  `failed` at once, and the run's own ending turns it into `Failed { red_check }`. Filing
  the exit there instead would publish a terminal update mid-run, breaking F1e's one
  ending per run, and would let a revoked child read as ended and dodge its grace.
- Stopping a run is `abort` and a bounded join (10 s); `abort` already kills bash and
  interrupts the kernel cell. No separate process-group kill was added.
- `close` is called where a parent's end is an event the process survives (ACP's session
  handle drop). The CLI and TUI exit paths end the process and are unchanged.
- `Failed { class }`: `refused_spawn`, `provider` and `deadline` have producers in
  `run_child`; a child's own `failure` envelope is filed as `red_check`; `kernel_death`
  has none until the kernel-dead path reports it.
- Audit item (e): `fold_event`, `preview`, `update` and the new transition moved to
  `subagent/record.rs`, the lease and the two stop registrations to `lease.rs`;
  `subagent.rs` stands at 1,130 lines
  and `session.rs` at 1,198. Nothing else was shed.

### F3a · The judge tier (D216, landed 0.276.0; extends D194, D215)

**Files.** `crates/runtime/src/plan/judge.rs` (new: the envelope of §6.4),
`verify.rs` (`Judge` bound), `subagent.rs:1012` (`find_models` filter by
family), `fetch/log.rs:82,167` (quote check), `extract.py`.

| control | test (tier) |
|---|---|
| a judge of the owner's family is never used | `judge::a_judge_is_another_family_or_the_item_abstains` (T0) |
| the schema is the only accepted answer | `judge::malformed_or_empty_answers_abstain` (T0; the `auto_review.rs:62-81` pattern) |
| an unbacked quote abstains | `judge::an_unbacked_quote_abstains_the_item` (T1, transcript fixture) |
| the jury cap escalates | `plan_ops::the_fourth_jury_on_one_todo_escalates_to_the_user` (T0) |
| the judge sees no owner transcript | `judge::the_brief_carries_no_history_or_prior_verdict` (T0) |
| one pass and two abstentions abstain under the n = 3 quorum | `judge::one_pass_and_two_abstentions_abstain` (T0) |
| instructions inside the evidence change nothing | `judge::evidence_carrying_instructions_is_data` (T1, the `auto_review.md:5-9` rule) |
| a full worker set still gets its jury | `judge::full_capacity_still_adjudicates_through_the_reservation` (T1) |
| a judged item never stands alone | covered by F0c's floor test; re-asserted for a live `judge` decider in `Contract::validate` (T0): `contract::validate_enforces_the_floors_and_a_judge_never_stands_alone` |
| a brief naming context its wall denies is refused (handed down by F2b, §7.6) | `judge::a_brief_naming_context_its_wall_denies_is_refused` (T0) |

LOC yi-runtime +320. Memo: `growth +320: the judge envelope`. Row: "A judged
contract item is a walled reader of another model family answering a fixed
schema, its quotes checked against its fetch log, aggregated in Rust
(D-next-10; Closes #<n>)". ADR: "D-next-10: the judge tier is an envelope".

As landed (0.276.0, D216, #460). Measured growth is +578 Rust `src` lines (yi-runtime
534 against 320, yi-types 44) and 7 of Python. The tests are `tests/judge.rs`, so
`judge::` names that file; the F0c floor test was renamed
`validate_enforces_the_floors_and_a_judge_never_stands_alone`. What the estimate did not
price: the seat (the verification permit carried from the done path through `Snapshot`
into `spawn_seated`), the jury count and the escalation arm in `done.rs`, the juror lines
with their session-visible counts, and F2b's context refusal.

- The cap lives in `Verifier::run`, not in the jury, so the engine's test drives it with
  a stub `Judge` and no host, beside the whole-verification deadline, which is read once
  ahead of every decider so that no jury is seated for an item with no time left. An
  `Escalate` outcome blocks the todo on the user on that refusal, under a note of its own;
  before F3a nothing produced one.
- `find_models` moved to `subagent/models.rs` beside `family_of` and `other_families`
  rather than gaining a filter argument; `subagent.rs` is 6 lines smaller.
- `extract.py` reads `jurors` counts from the `done` and `done_refused` session records
  as `juror_pass`, `juror_fail`, `juror_abstain` and `quotes_dropped`; quorum outcomes
  and escalations were already `verdict_*`.
- Not built. A jury on the worktree paths: `submit`'s candidate check and the staging
  check pass no seat, so a judged item abstains there; one jury on each would spend two
  of a todo's three on a single submit, which wants a decision about which check the
  jury belongs to. A `judge(...)` builder in the `yi` library: a judged item is declared
  through the contract's JSON, and §6.4 puts calibration before default use. A `judge`
  record kind and a `todo.juries` field (both derived instead). A fragment or paged read
  as backing for a quote. A configured judge model. `Disposition::RepossessionPending`
  (see F2b's list). Calibration on a labelled set, which §6.4 requires before a judged
  item is used by default, has not been run.
- Limits a deployment reaches before any of that. A `plans.dir` outside the cwd puts the
  evidence blobs outside the tree a juror reads, so every judged item abstains there; a
  host whose own grant is smaller than three juror leases of 100,000 tokens has a seat
  refused and abstains by quorum; and a todo the plan holds no delegation for, which is
  every todo the owner ran itself or spawned by hand, contributes no owner beyond the
  host's own model, because nothing records the model a child actually executed on
  (`spawn_result` carries the agent name only). Each abstains or narrows with its reason
  in the verdict, none is silent, and the owner of all three is F3b, which is where a pod
  records who read what.

### F3b · Review pod with a code arbiter (D217, landed 0.277.0; extends D212, D216)

`yi/recipes/review_pod.py`: N readers with distinct briefs (correctness,
tests, scope) and one arbiter todo whose contract is a `cmd` (the checker) or
an `example` set; the pod's verdict is the arbiter's, the readers' findings
are evidence attached to the todo's `note`. Tests: `test_yi_shapes::a_pod_verdict_is_the_arbiters_command_not_a_reader`
(T0); a journey `review_pod_on_the_fixture_repo` (T2). LOC python +200.

As landed (0.277.0, D217, #461, and the review fix on top of it). Python +128 against 200,
no Rust `src` line; tests +62 of Python and the journey in `kernel_data_surface.rs`, whose rig moved into a `crewed`
helper the F1b journey shares. The T0 test carries the unittest prefix,
`test_a_pod_verdict_is_the_arbiters_command_not_a_reader`, and a geometry test sits beside
it, `test_a_pod_without_a_code_arbiter_or_distinct_briefs_is_refused`.

- The recipe is `declare` plus a module-level `review_pod(plan, run)` built on
  `shapes._schedule`, `_survivor` and `_asked`; there is no third scheduler. A finding is
  scatter's `ANSWER` (one answer and its quotes), so it passes the same quote seam, bound
  to the reader's own partition; a null answer is "nothing found".
- "Attached to the todo's `note`" is the delegation's note, and no op rewrites a declared
  todo, so the arbiter is issued again once the readers settle, as `<key>-r2` (scatter's
  round naming), with the findings in `delegation.note` inside the 1 KiB `InlineNote` cap
  and each reader's whole answer as a `context` url; the declared arbiter is dropped. The
  arbiter is therefore a delegated todo: an inline or an owner-run one has no delegation to
  carry a note.
- The arbiter is started once and never retried, and geometry refuses a `judge` item on it,
  so a pod spends no jury and issues one verification per arbiter.
- Not built: one_for_all on the passes (readers get `_schedule`'s one_for_one, and a reader
  that stays failed is dropped and named "no backed finding"); a calibrated findings
  contract; and the record of which model a reader ran on that F3a's limits hand to this
  stage. The recipe sees only the role's declared `model` and `rlm.result` carries none, so
  that record needs the host's spawn result to name the model, which is a wire change this
  stage did not make. F3a's other two limits (a `plans.dir` outside the cwd, a grant
  smaller than three juror leases) are untouched by a pod, which seats no jury.

### F3c · Services with stable addresses (D218, landed 0.278.0; extends D165, D214, D215, D216)

`rlm.service(name, brief, restart=…)`: a child whose name is reserved; a
respawn keeps the name and the inbox; `status()` shows `service: true`;
`history://<name>` spans respawns through the kept transcript chain
(`fetch/mod.rs:190-200`). Tests: `recursion_e2e::a_service_respawns_under_its_name_and_keeps_its_inbox`
(T1). LOC yi-runtime +140, python +40.

As landed (0.278.0, D218, #462, and the review fixes on top of it). Measured growth is +430
Rust `src` lines (yi-runtime 424 against 140, yi-types 6) and 24 of Python against 40. What
the estimate did not price: a service lives in turns mail wakes, which no `run_child`
watches, so a second road reads those endings; the lease is settled and drawn again under
the roster lock, and journaled off it; the stopped mark is read again after the build;
attach or refuse; and the `rlm.service` registration. Tests beside the named one, all
`recursion_e2e` (T1): `a_service_out_of_restarts_or_lease_ends_failed_and_says_so`,
`a_service_the_parent_cannot_relend_ends_failed_and_says_so`,
`a_woken_crash_respawns_and_a_parent_close_ends_a_service_for_good`,
`a_service_revoked_while_its_next_run_is_built_never_comes_back`,
`a_respawned_service_is_billed_from_its_own_first_turn`,
`a_service_is_outside_the_worker_cap_and_under_the_depth_limit`.

- A respawn reuses the record: the new session is attached to the same session store and
  `Step::Respawn` clears the exit, so the name is never free, the inbox is the same file,
  `history://<name>` is one chain with no change to `fetch/mod.rs` or `kept_transcript`,
  and no terminal update is published for a run that was respawned. The respawned session
  therefore loads its predecessor's transcript, and its brief opens with a line naming the
  incarnation and the cause.
- `restart` is the intensity: how many respawns are allowed inside ten minutes
  (`RESTART_WINDOW_MS`), default 3, 0 for none, and `MAX_RESTARTS` (ten) is the most a
  caller may ask for, refused and never clamped. Only a provider error and a dead kernel
  respawn; a deadline does not, because a fresh lease on expiry would undo the lease.
- `turn_ended` lets one reader at a time out on a service's run: mail can start a turn on a
  run that already crashed, and two readers of that one crash would end it twice, on two
  leases and two incarnations. The race needs two `AgentEnd`s inside one crash window, which
  the harness cannot schedule, so the guard has no test of its own.
- The build is the one stretch a respawn holds no lock across, so the stopped mark is read
  again under the roster lock after it, and a service a `revoke` or a `close` stopped while
  its next run was being built ends `Failed` with that as the reason; the session that build
  produced has its kernel disposed rather than dropped, as the dead incarnation's does.
- Each incarnation is billed for its own turns: `ChildRecord.billed_from` is the kept
  transcript's length at the respawn, and `refold` and the lease return both count from it,
  so a lagged watch cannot charge a predecessor's turns to the successor's lease and a
  provider error's unknown usage stops spending reservations after the one it ended.
- `ChildRecord.juror` became `Standing { Worker, Juror, Service }`. A service is outside
  the worker cap and sends no notice for an idle turn; it is under the depth limit, the
  family cap, the lease and the wall, and takes no verification seat.
- Section 7.1's `from_incarnation: u32` landed as `Option<u32>` like `to_incarnation`, so
  an envelope between members that are no service is byte for byte what F2a wrote. The
  waiter carries no incarnation (section 7.3 asks for one): the respawn retires every
  waiter on the name through `drop_respondent` before the successor can reply, which is
  the same refusal one step earlier.
- Deliberate stops: `delete_subagent` removes the record, `revoke` and `close` mark the
  service stopped (`close` reaches an idle service too, which holds no run to revoke). The
  `revoke` mark is tested where it is the only thing that can act: a crash whose respawn is
  already building, in `a_service_revoked_while_its_next_run_is_built_never_comes_back`.
- Not built: the health `cmd` and shutdown contract of section 6.2's service row, a
  service in a worktree (refused by name), and adoption after a host restart (counts and
  incarnations restart with the host).

### F4a · The procedural graph replaces affordance strings (D219, landed 0.279.0)

**Files.** `crates/types/src/graph.rs` (new), `crates/runtime/src/prompts/graph.json`
(new), `affordance.rs` (renderer; stays under 300 lines), `todo/text.rs:204-230`
(the todo producer migrates), `tools.rs:305-308` (facts passed), tests
`affordance.rs`.

| control | test (tier) |
|---|---|
| the shipped graph is well formed | `affordance::the_shipped_graph_passes_every_structural_check` (T0) |
| migration is byte-identical | `affordance::every_line_rendered_today_renders_from_the_graph` (T0, the existing string tests kept) |
| localization is exact match | `affordance::an_unknown_last_call_renders_nothing` (T0) |
| two hops and three lines | `affordance::rendering_stops_at_two_hops_and_three_lines` (T0) |
| conditions are the closed set | `graph::an_unknown_predicate_fails_to_parse` (T0) |

LOC yi-types +90, yi-runtime +260 −80. Memo: `growth +270: the procedural
graph and its renderer; the affordance strings became data`.

As landed (0.279.0, D219, #463). Measured growth is +207 Rust `src` lines against 270:
yi-types 180 against 90, yi-runtime 27 net against 180 (121 added, 94 deleted). The tree
settled four things this section left open. The predicate set lives in
`crates/types/src/graph.rs` as the table `PREDICATES`, and `Predicate` is a string that
parses only against it, so an unknown condition fails where the graph is parsed and
`graph::an_unknown_predicate_fails_to_parse` is a yi-types test; the host asserts the
predicates that hold as facts (`Facts.holds`) and the renderer compares names, which is
why `MemberState`, a runtime type, never had to move. The old producers branched on
states section 9.2's list did not name, so the closed set gained `todo_state(...)`,
`coroutine_unawaited`, `method_awaited`, `listing_name_missed`, `grid_answer_empty`,
`session_on_disk` and `session_in_memory`; `spawned` and `child_finished` became
`child_state(running)` and `child_state(finished)` on `rlm.run`, because two `always`
edges on one node would have rendered both lines where one rendered before. The todo
producer localizes at the tool (its lines are the same after every op) and renders one
item at a time, so the cap of three stays `NEXT_LINES` in `next_lines`. The seven string
builders and the `moves` match were dead once the goldens passed from the graph and are
deleted; section 12's value row is still owed before the graph is more than an
equivalent. Seven predicates are vocabulary no seam asserts yet (the row lists them).

### F4b · The offline refiner with rejection memory (part of D219, landed 0.280.0)

`evals/graph/refine.py`, `evals/fixtures/graph/{proposals-sample.jsonl,
rejected.jsonl}`, `evals/levers/split.json` (shared with F4c). Tests (stdlib
unittest under `evals/`, run by `selftest.py`): `a_graph_edit_that_drops_the_held_out_score_is_rejected_and_remembered`;
`a_proposal_naming_a_held_out_task_is_refused`; `a_structurally_invalid_edit_never_reaches_a_run`.

As landed (0.280.0, #464; no Rust `src` line). The scoring run is an injected callable:
`refine(graph, edits, split, run, config)` in tests, the owner's `--runner` command on the
command line, so `refine.py` itself starts nothing; section 9.5's `ab.py` and the paid slice
are what a runner wraps. The development tasks filter before the held-out tasks are touched
(`fit_passes_dropped`), which keeps held-out accesses to candidates that earned one. A tie in
which neither graph solves anything prices no token, so section 9.8's byte rule decides it
(`guidance_bytes_unpaid`). The refusal of section 9.6 covers validation as well as final
tasks, and reads the decoded line by token, so a JSON escape hides no id and an id inside a
longer word refuses nothing. The two rule sets are held
together by `evals/fixtures/graph/structural.json`, judged by `graph::the_shared_fixture_is_judged_alike_on_both_sides`
and by the Python test of the same name. `split.json` is drawn over the seven synthetic
tasks of `evals/fixtures/tasks` (development four, validation three, final empty); the
thirteen-task slice of section 10.4 is benchmark data and stays out of the split file until
the owner decides how it is named there. `graph.json` is kept in the writer's one-line-per-edge
form. Not run: any real scoring.

### F4c · Levers manifest, floors, the two gates (D220, landed 0.281.0; extends D140)

`crates/runtime/src/levers.rs`, `evals/levers/{levers.json,default.json,floors.json,split.json}`,
`evals/levers.py`, `env_vars.json` (`YI_LEVERS`, own `Ratchet:` commit).

| control | test (tier) |
|---|---|
| both sides agree | `levers::the_default_fixture_equals_the_compiled_defaults` (T0) and `levers.py --selfcheck` |
| eval mode only | `levers::without_yi_levers_the_defaults_are_used_and_the_file_is_never_read` (T0); `levers::yi_levers_set_outside_eval_mode_is_ignored` (T0) |
| a lever that improves cost by failing the floor is rejected | `evals/tests/test_levers.py::a_cheaper_candidate_below_the_floor_is_rejected_with_its_class` (T0) |
| survivors are nondominated | `test_levers.py::a_dominated_candidate_is_not_a_survivor` (T0) |
| the held-out split never enters the fit | `test_levers.py::fit_refuses_rows_that_name_a_held_out_task` (T0) |

LOC yi-runtime +240 (the struct, the reads at each constant), evals +400.
Memo: `growth +240: the kernel's constants read through one Levers struct`.

As landed (0.281.0, #465; src +178). The tree won over section 10.1's table: `plan.nudge_cap`,
`todo.artifact_steer_turn` and `todo.artifact_cap` no longer exist (D182) and are not listed,
`loop.cut_stop_at` is 6 and `lane.slots` is 255 (grow on demand). 45 levers are listed and 25
are tunable; the other 20 carry a `why`: the two fuses, the six mail caps, the ladder's height
(three rung texts), the jury size (a contract rule), `family.depth`, `lane.slots` and
`advisor.cadence` (config keys already), and the constants of yi-loop, yi-tools and
`shapes.py`, which no `Levers` read reaches; wiring one of those is its own change. `Levers` is
a process-wide `OnceLock` read through `levers::get()`, not a `RuntimeWiring` field: the reads
sit in pure methods (`Cycle::work`, `Rung::delay`, `Features::route`) that hold no wiring, and
one process runs one configuration. The tree had no eval-mode flag, so eval mode is the flag
both runners already pass and no default sets, `--deadline`. The manifest's ranges live in
Rust too, since the loader cannot read `evals/` at run time; `levers::the_manifest_matches`
holds the two equal. `floors.json` carries the `fixtures` class only: the first-cut classes of
section 10.3 name benchmark tasks and wait for the owner, as the split does. The override
file's hash rides the fingerprint's mode in `evals/run.py` (section 10.6); the harbor adapter
does not carry `YI_LEVERS` into its container yet. Not run: any paid comparison.

### F4d · Controlled comparisons over three to five knobs (part of D-next-14)

`levers.py compare` (paired baseline and candidate over one knob) and
`levers.py grid` (a bounded grid over the chosen knobs, every run counted in
the search spend); tests on synthetic rows: `a_comparison_reports_its_interval`,
`a_grid_refuses_a_knob_marked_not_tunable`,
`a_candidate_outside_its_range_is_refused`. No OLS over the manifest and no
Gaussian process (§0 R12, §10.4); a later optimizer is its own proposal with
the data to justify it. Paid runs are the user's; each promotion is a ledger
row and a changelog row naming it.

### Deferred (seams only)

Hive (`placement` null on `Delegation`; the capsule manifest as a documented
format), A2A adapters, module regeneration.

## 12. Migration and deletions

| what | where | when |
|---|---|---|
| format 1 → 2 by an explicit `import` op; the original bytes kept as an artifact blob, the `.md` left until a confirmed cleanup | `plan/import.rs` | F0b; the reader stays two releases |
| `fold_user_edits`, `known` | `ops.rs:515,547-563,548-551` | F0b (replaced by digest detection against the journal, never blind trust) |
| `user_edits`, `HandEdit`, `running_by`, `PlanFile.body` as mutable state, the frontmatter writer | `store.rs:65-146,259-260,376-392` | F0b (`split_frontmatter`, `parse_document`, `DocumentError` survive two releases inside `import.rs`) |
| `python/skills/plan/` and its `PYTHON_SKILLS` row | `bootstrap.rs:274` | F0a, in the PR that ships `plan.op` |
| `do_set`'s direct `TodoState::Done` construction | `ops.rs:1035` | F0c (every transition through the validator; the construction survives behind it for the uncontracted carve-out D194 records) |
| `Delegation.accept: Check` as live authority; `red_count`, `red_fingerprint`, `readmit` | `doc.rs:196,60-65`, `plan.rs:63-71` | F0c (`accept: command` still rides the spawn as the child's own check and `accept: stated` only raises strictness, D194; the fields land in `extra` with wire fixtures) |
| the `notice` wiring of the child host (`notice_hook` itself stays for its five other callers) | `wiring.rs:603` | F0a (to `wake_idle_hook`) |
| `RLMSpawnHandle.result`'s 0.5 s poll | `rlm/__init__.py:52-83` | F0a (cursored `wait`, same timeout and errors) |
| `take_pending`'s shared drain | `mailbox.rs:304-319` | F0a (per-caller cursors; `updated` kept one release) |
| `admissible` (replaced by `admit`) | `table.rs:227-237` | F0b |
| the write-then-emit order of `framed` | `ops.rs:535-541` | F0b (journal first) |
| `Plan.replay` (never built) and any cell re-execution | plan §8.3 | removed; `Plan.resume` instead |
| the nine affordance producers and `todo/text.rs::moves` | `affordance.rs:9-84`, `todo/text.rs:204` | F4a, only after byte-identical migration and a value row |
| the OLS and GP sweep over the manifest | plan §10.4 | removed; three to five knobs |
| `docs/YI_DESIGN.md` §8.17.1 (:1211-1246): "a done claim is host-verified" becomes true again with the contract sentence, no wider than the tested boundary; :1220-1222 carried; §8.10 (:869-886) gains the lease, the wall on the spec and the envelope | docs | with each stage's row |
| `docs/YI_DESIGN.md:1260,1282-1284` event vocabulary lines | docs | F0a notes the count stays fourteen |
| the walkthrough fixtures rewritten as programs beside their JSON | `python/yi_runtime/tests/programs/` | F1b |
| `orchestrate.md` examples, `doctrine.md` rules 3-5, `identity.md` | prompts | F0a (+20 B), F0c (+40 B), F1d (`--update`) |

## 13. Decisions: recorded and open

Recorded 2026-09-13 by the owner: the journal stays in `.yi/plans/<slug>/`
(`ops.jsonl` the commit point, `plan.json` the checkpoint; §0 S1); the
original format-1 bytes are kept as an artifact blob, never a Markdown
document the store reads (§0 S3); F1b ships `fork_join` and `scatter`
(§0 S4); the F0e value gate's tolerance is 10 percent on cost and median
wall (§0 S9).

Open, each with the default the plan assumes:

1. **Milestone name and issue prefix.** Default: milestone "An operating
   system for agent work", issues titled `F0a …` through `F4d …`.
2. **Note cap on import.** Default 4 KiB per todo inline; larger sections
   become artifact references, never a cut.
3. **`ops.jsonl` in git.** Default ignored by a store-written `.gitignore`
   line; `plan.json`, `program.py` and `artifacts/` tracked (D97); the
   root's backup procedure includes the journal.
4. **`DONE_REFUSAL_CAP` and `JUDGE_CAP_PER_TODO`.** Default 3 and 3; both
   levers; infrastructure and staleness refusals accounted separately.
5. **`on_parent_close` default.** `Terminate { grace 30 s }` with
   preservation; `Abandon` refused; F2b's first test pins today's behaviour.
6. **Task classes for floors.** Default the three groups in §10.3 plus
   `fixtures`; the owner may regroup before the first run.
7. **The examples runner protocol.** Default stdin JSON in, one JSON value
   out, structural comparison; no per-language harness.
8. **The judge's family rule.** Default the vendor segment of the recorded
   model identity; none distinct → abstain.
9. **The `fan-out` gate task.** Default: written in harbor layout, two
   deliverables with stated checks, run paid in the F0e row.
10. **`YI_LEVERS` as the one env var.** Default yes, and only when an
    explicit eval mode is also on; production ignores the variable.
11. **The confirmation UI for administrative ops.** Default: the console's
    existing permission prompt, bound to `{root, op, argsHash,
    expected_revision, expiry}`; the TUI reuses it.
12. **Which three to five levers the first sweep may move.** Default:
    `plan.done_refusal_cap`, `family.stuck_idle_s`, `plan.width_max`,
    `todo.nudge_work`.

## 14. Killed on the way

- A third `Actor` for the CLI: `Actor::User(Url)` already carries the
  citation; the CLI is a user.
- Storing full `Op` args in the session sink too: the session record stays
  slim; `ops.jsonl` carries the args for replay.
- A `plan.create`/`plan.update` compatibility shim for the dead skill: nothing
  registered them; there is nothing to be compatible with.
- Keeping `Check::Stated` as a live decider: a stated acceptance is a
  legacy unverified requirement; it completes only by a decidable item or
  a user's acceptance.
- A separate `custom_type` for envelopes: `agent_message` with `details`
  keeps every counter that exists.
- Exactly-once mail, pub/sub topics, contract-net bidding, cross-root mail:
  cut in §3.4.
- Holding the store lease across the verifier: a ten-minute lease starves
  every other op; the re-read at step 5 is cheaper than a long lock.
- A `mail.inbox` syscall: `history://<self>/since/<seq>/custom/agent_message`
  is the inbox.
- A `graph.next` syscall: rendering is host-side at the seam.
- An LLM in the graph's condition set: the predicates are a closed enum.
- An OLS fit and a handwritten Gaussian process over the levers manifest:
  48 knobs against 40 rows is underdetermined; three to five knobs and
  controlled comparisons instead (§0 R12).
- `Plan.replay` and any re-execution of recorded cells as recovery: source
  is an audit artifact; `Plan.resume` reads durable state (§0 S5).
- A one-line `heartbeat_hook` wake: it loses a notice at `AgentEnd`
  (`hooks.rs:82-83`); `wake_idle_hook` instead (§0 R8).
- "A grace fires within a second": the probe loop sleeps up to 60 s
  (`probe.rs:233-258`); a `Notify` and an observed latency instead (§0 R9).
- A blanket refusal to reap an unmerged worktree: it blocks `fail`, `drop`
  and `supersede` (`ops.rs:699-708`); dispositions instead (§0 R7).
- `put(label)` as the inline output path and `family://` as its product:
  the name rule and the sidecar refuse both (§0 R10); artifact ids instead.
- Argv as the user's authority: an agent's `bash` runs the same binary;
  a confirmed channel instead (§0 S2).
- Relocating the journal outside the checkout (§0 S1) and keeping the
  original `.md` as a store document (§0 S3): rejected by the owner.
- Deleting the `.md` on import and the format-1 reader after one release:
  lossless import, two releases (§0 S3).
- Growing `ops.rs`, `subagent.rs`, `mailbox.rs` or `todo/coupling.rs` in
  place: each is within 150 lines of the 1,200 cap (§2 B21); new files.
- Renaming `rlm` to `child`: deferred by the user (`2026-09-08-pass-levers.md:658`).
- A `Scheme::Family`/`Scheme::Tree` variant: both ride `External` today
  (§2 B16) and nothing here needs the variant.
- A held-out judge model list in config: the family rule needs no list.
- A `verification economy`, dry runs, a shared-tree lint (§3.4).

## 15. Appendix

### 15.1 Prior-art table

| primitive | source | what it contributes |
|---|---|---|
| admission as refusal, visible revocation, abort protocol, repossession vector | Exokernel (Engler, Kaashoek, O'Toole 1995) | §3.3 admission, §7.4 leases, §7.6 walls |
| disposable loop templates, two gates, nondominated survivors, held-out never in the loop, token cost | SoL-Pi (NVIDIA 2026) | §8.4 recipes, §10.3 gates, §10.5 |
| readers decoupled from reasoning, partitions, abstentions dropped, rounds | ParSer (CUHK 2026) | §8.6 scatter |
| typed procedural triplets, exact-match localization, two-hop rendering, offline evolution with rejection memory | Procedural Graphs (Google 2026) | §9 |
| humans supply goals; the human inbox is the bottleneck; an automated reviewer catches a third | Anthropic, When AI builds itself (2026) | §6.3 escalation to the user, §11 F3b |
| binary checklists, abstention with coverage, diverse panels, blinding | PaperBench, HealthBench, CheckEval, Trust or Escalate, PoLL | §6.1 items, §6.4 judge |
| as-needed decomposition, selective retry, specification failures | ADaPT, RSTD, MAST | §8.5 restart strategies, §6.2 floors |
| span 4-5, larger teams lower the KPI, superlinear coordination | Graicunas, MultiAgentBench, Nature MI 2026 | caps 8 and 16 kept as fuses, §10.1 |
| envelope fields, per-pair order, monitors, restart intensity, at-most-once, signals/queries/updates, parent-close | FIPA ACL, Erlang/OTP, Akka/Orleans, Temporal | §7 |
| checkpoint recovery, aligned rewind | Crab, AgentRewind | §5.6 repair (reconstruction, then reconciliation) |
| a commit record, flush ordering, checkpoints, explicit crash assumptions | SQLite atomic commit and WAL (reference mechanisms, not a validation of Yi's journal) | §5.3 |
| panic budget, newtypes, durable writes, outcome modelling, two-phase shutdown, STRIDE per boundary, the verification ladder | the `har-*` skills (`~/Development/har-skills`), distilled from High Assurance Rust, Effective Rust, Rust Atomics and Locks, the tokio docs | §3.7, §5.3, §6.1, §6.3, §7.4, §11 |
| orchestration history separate from activity results; execution identity; retry and idempotency as the activity's problem | Temporal activities and workflow execution (the distinction, not the product) | §5.3 `spawn_intent`, §8.3 resume |
| capsule manifest, laws L1/L4/L6, placement, test the control | `docs/plans/2026-09-01-hive.md` §7.4, §7.5, §8, §11 | §3.4 seams, §7.6, §11 tables |
| the ledger's yield: `yi why`, Amdahl, discovery ratio, fuzz | `docs/plans/2026-08-31-four-primitives-and-a-plan-engine.md` §12 | §4.4 views, §10 |

### 15.2 Research anchors, with the numbers as given

Exokernel: separate protection from management; secure bindings, visible
revocation, abort protocol with a repossession vector; expose allocation,
names, revocation; downloaded code must be bounded; the lower the primitive,
the more latitude above it. SoL-Pi (NVIDIA 2026): disposable loop templates
beat a growing coordinator; two acceptance gates with nondominated survivors;
held-out never enters the loop; Action Fusion, ObservationPack,
Evidence-Preserving Reducer, Online Context Compact; one idea in forty
survives; a swarm shares evidence, not every thought; 94 percent of Pi's
score retained at 45-49 percent fewer tokens. ParSer (CUHK 2026): decouple
reading from reasoning; readers bound to disjoint partitions, frozen, cheap
(4B saturates), abstentions dropped; scatter-gather rounds scale with
reasoning depth, 11x lower latency at 896K tokens; only the lead is trained.
Procedural Graphs (Google 2026): typed (procedure, relation, procedure)
triplets with condition, guidance, pitfalls; localize by exact match on the
last tool call; two-hop guidance cut tokens up to 70.9 percent versus the
full graph; frozen online, evolved offline under a validation gate with ties
accepted and rejection memory; evolution from scratch beat and repaired
expert priors; 81.8 percent fewer tool calls on EnterpriseArena; guidance
still costs 33-55 percent more tokens on some tasks. Anthropic, When AI
builds itself (2026): humans supply goals and choose problems; an automated
reviewer would have caught about a third of incident bugs; steering beats
the human 64 percent at off-course moments and 20 percent where the human
was strong; human review is the Amdahl bottleneck; task horizons double
about every four months. Judging: PaperBench weighted binary leaves F1
0.83; HealthBench criterion grading at physician agreement; CheckEval +0.45
agreement with binary checklists; Trust or Escalate above 80 percent human
agreement at about 80 percent coverage with abstention; PoLL diverse
three-model panel 7x cheaper than one frontier judge; position bias
systematic above 0.85 repeatability; self-preference recall gap 0.52;
LongJudgeBench mean judge accuracy 0.56; WebDevJudge best model 70.3 percent
with humans; Gao 2023 proxy overoptimization. Decomposition: ADaPT +28
points as-needed; RSTD static decomposition +80.5 percent retry tokens,
selective retry −51.7 percent; Planetarium 96.1 percent parseable, 24.8
percent semantically correct; MAST 41.8 percent specification failures;
self-verification false-negative rate 95.8 percent on Graph Coloring.
Organization: Horling and Lesser catalogue; Graicunas span 4-5;
MultiAgentBench larger teams lower the KPI; Nature MI 2026 coordination turns
grow superlinearly (exponent 1.72). Messaging: FIPA ACL envelope fields;
Erlang per-pair ordering, monitors, supervisor restart intensity; Akka and
Orleans at-most-once default; Temporal signals, queries, updates,
parent-close policy; Crab checkpoint recovery 100 percent versus 8-13
percent; AgentRewind 62.2 to 87.8 percent with aligned rewind. Yi's own
numbers: row 0025 (`docs/eval-ledger.md:63`) 3/18 at k=3, $0.77,
`children_spawned` 0, `kernel_dead` 0, `kernel_cells` 87; rows 0018-0023:
77 pointers emitted, 0 read (D161); the kernel dead in 8 of 9 calls (D160);
`orchestrate.md` attached in 158 of 158 sessions.

## 16. Refute log

Claims handed to this plan that the tree refuted or narrowed, one line each,
all corrected above.

- `admissible` takes the first N ready todos: no; undelegated todos bypass
  the slots (`table.rs:227-237`). §3.3 and §5 describe `admit` accordingly.
- `Actor::User` alone may Unblock or View: `Actor::Host` shares the arm
  (`table.rs:162`); the probe's unblock is the second production writer
  (`probe.rs:198`), so "mutations reach the engine only through the tool"
  was wrong.
- `stop_posture` is Quiet while a child runs: true for the todo tool
  (`todo/coupling.rs:331-333,340`), false for the plan coupling
  (`loop_coupling.rs:121-125,134-137`), where a running child sets nothing
  and Blocked{Child} is Continue.
- `AgentEvent::Custom { details }`: no such variant; the envelope's carrier
  is `AgentMessage::Custom` (`message.rs:237-244`).
- a vocabulary-cap test: none exists; the cap is design-time
  (`docs/YI_DESIGN.md:1260,1282-1284`) and `AgentEvent` has fourteen variants.
- `family://` and `tree://` are schemes: both ride `Scheme::External`
  (`fetch/mod.rs:402-403`).
- `children_spawned` at `extract.py:95`: it is at :389-397 and misses
  engine-dispatched children; §10.6 adds `plan_children`.
- the request-prefix gate is one script: it is three (bytes, await-ness,
  the name allowlist in `ext_e2e.rs:332-367`).
- `PlanRepr`/`TodoRepr` live in `store.rs`: they live in `doc.rs:315-340,481-499`.
- `OpSink` lives in `ledger.rs`: it is `ops.rs:255-257`; `PlanOpRecord` is
  in `crates/types/src/plan/ledger.rs`.
- the lease is a pid file: it is a lock directory with a per-holder hold
  file (`store.rs:11-15,162-163,320-348`).
- `Report` at `ledger.rs:173`: the struct is :76-83; :173 is the function.
- `help(yi)` or `help(rlm)` exists somewhere: nothing does; §8.7 builds it.
- `.yi/schemas` exists: it does not; §5.2 publishes it.
- a slice file and a held-out split exist: neither does; the slice is a
  shell variable (`tbv4_baseline.sh:10`); §10.4 creates the split.
- `children_spawned` is a ledger column: it is prose in the notes cell
  (`docs/eval-ledger.md:63`).
- `merge_into` ends at :951: it runs to :955 and commits uncommitted work
  first (:939); §7.4 hoists that rule into `Lane::settle()`.
- the delete guard is :852-859: :851-858. `heartbeat_hook` :57-79: :57-80.
  `state_from_records` :136-161: :136-160. `apply` :399: :395 (the lease
  is :399). `do_start` :774: :759. `check_actor` :157: :159. `RETRY_CAP`
  :187: :186. `STEPS` :37: :38. `Delegation` :191: :194. `Plan` :455: :457.
  `Working model` :312-352: :312-367.
- a `plan.create` compatibility path is needed: the skill's names were
  never registered, so nothing depends on them.
- Cargo workspace version is the tree version: `Cargo.toml:22` says 0.2.0;
  the header (`docs/ARCHITECTURE.md:4`) is 0.207.0 and is what rows key on.
- what happens to children when a parent closes: not pinned by any test the
  recon found; §7.4 marks it speculation and F2b pins it first.
- `plan.schema.json` counts toward src growth: it does not; the growth gate
  globs `.rs` under `crates/*/src` and stops at `#[cfg(test)]`
  (`_common.py:10-18`).
- `notice_hook` would lose its last caller at F0d: it has five others
  (`session.rs:209`; `wiring.rs:324,525,567,650`); only the child host's
  `notice` wiring at `wiring.rs:603` moves.
- the `history://` filters are query parameters: they are path segments
  `tail/N` and `since/S` (`fetch/schemes.rs:30-44`); the inbox address is
  spelled accordingly.
- the probe ladder ticks every 60 s: its loop sleeps to the next due time,
  at least 1 s, and 60 s only when nothing is due (`probe.rs:244-260`); the
  stuck job and the revoke grace register due times on it.
- `agent_message` entries already carry `details`: they carry `None`
  (`mailbox.rs:68`), which is why the envelope can ride there without a new
  `custom_type`.

Corrected by the Revision 2 disposition (2026-09-13; §0 has the table):

- a verifier in `do_done` alone closes completion: `do_set` writes `Done`
  directly (`ops.rs:1035`); every path goes through one validator.
- `do_done` validates a declared output whenever one is declared: it skips
  validation when the product or the schema resolves to `None`
  (`ops.rs:836-838`) and when no resolver is attached (:822); an unserved
  side is now a refusal.
- a second sink makes the log and the snapshot atomic: `framed` writes then
  emits (`ops.rs:535-541`); the journal is the commit point and `plan.json`
  its checkpoint.
- `do_start` persists before it spawns: the spawn runs inside `mutate`,
  before `write_all` (`ops.rs:535-539`); `spawn_intent` precedes it now.
- critical abstentions leave the denominator: they made a pass possible
  with an unavailable critical judge; any critical `Abstain` abstains.
- the re-read at done checks enough: it checked legality, not identity; the
  verification token is compared whole.
- the judge's history block is `deny_read`: `deny_read` is paths and
  `deny_url` is prefixes (`wall.rs:10-14,60-64`); `deny_url` now, and every
  wall is cooperative (`wall.rs:114-115`).
- a reap guard on unmerged worktrees is safe: `step_todo` reaps on every
  exit from Running (`ops.rs:699-708`) and `take_settled_worktree` takes the
  lane on discard too (`lane/mod.rs:1033-1052`); dispositions instead.
- one line to `heartbeat_hook` fixes the wake: `wake_idle_hook`
  (`hooks.rs:82-96`) exists because that hook loses a notice at `AgentEnd`.
- two waiters can share `wait`: `take_pending` zeroes the shared counter
  (`mailbox.rs:304-319`); per-caller cursors.
- a grace fires within a second: `next_wake` caps the sleep at 60 s and the
  loop sleeps before it ticks (`probe.rs:233-258`); a `Notify`, and the
  latency is observed, not inferred.
- `put(label)` and `family://<label>` carry an inline output: `put` requires
  a file-safe token (`rlm/__init__.py:535-536`) and `family://` serves the
  sidecar (`schemes.rs:236-254`); artifact ids and a product resolver.
- argv attests a human: an agent's `bash` runs `yi plan fuse reset`; the
  confirmed channel mints `Actor::User`.
- an OLS fit over 48 levers with 40 rows, then a GP: underdetermined; three
  to five knobs.
- rows 0022 and 0024 prove the mechanism works: both 21/21 while cost and
  wall rose (`docs/eval-ledger.md:60,62`); activation is not value, and
  `children_spawned` 0 in row 0025 counts only cell text.
- replaying recorded cells is safe recovery: it is not; source is an audit
  artifact and `resume` reads durable state.

Corrected by the high-assurance review (2026-09-13; §3.7):

- the checker inherits nothing sensitive: `run_captured` sets no
  `env_clear` (`process.rs:110-127`), so a checker running repository code
  would see the session's environment, provider keys included; the verifier
  scrubs it and a test dies with the control.
- "append then write the checkpoint" is a commit: only with `sync_data` on
  the journal, `create_new` for the temp file, and a directory sync after
  the rename; the plan now names each step.
- a signal or `abort()` terminates a child: both are requests; repossession
  commits only after the join.
- ids as `u32` and `String`: newtypes with checked constructors, `Permille`
  and `Weight` bounded, division by a `NonZeroU64`.
- the release profile catches overflow: no `[profile.release]` block was
  found in `Cargo.toml`; checked arithmetic in the code, and whether the
  `dist` profile sets `overflow-checks` is verified at F0b.
- the workspace forbids `unsafe`: `[workspace.lints.rust]` carries no such
  line (`Cargo.toml:70-84`); the crate roots are checked at F0b, and the
  plan adds no `unsafe`.
- the console rpc peer is the user: the daemon listens on a socket, so a
  peer is anything on the box; confirmation stays with the process that
  owns the permission prompt, and the socket's trust is verified at F0b.

