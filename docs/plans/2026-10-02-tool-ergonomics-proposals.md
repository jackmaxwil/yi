# Tool ergonomics: eight proposals, pressure-tested against the tree and the session logs

```
status:  PROPOSAL, revision 3 (2026-10-03). R1, R2, R3, R4, R5, R7 and R8 are implemented in
         this lane, uncommitted, each with its test seen red first; R9 was measured and cut
         (section 2); R6 is cut (section 3); X1 waits for a paid eval. No forge issue is opened
         yet (D106). Revision 2 took a cold review (Cursor, 2026-10-02) that refuted R1's routing
         fix, the E2 correction and R6; each point was checked against the tree first.
tree:    main @ e53547c2, re-read for this document.
inputs:  2026-09-27-tool-dogfood-report.md (three dogfood runs of every tool) · the 38 top-level session logs
         under ~/.yi/sessions modified in the 14 days before the corpus pass (the review,
         later, counted 39), 19 of them with tool results, one of them the session that wrote
         this · probes run for this document on this machine: four red cargo builds, one red
         cargo test, nine question-children, a corpus pass in the kernel
marks:   each proposal ends in a verdict: do, do after, measure first, experiment, or cut
```

## 0. Summary

The dogfood runs asked for ergonomics. Measuring them turned up one correctness bug that matters
more than any ergonomic gap: **a red cargo build can reach the model with its error removed,
and the cut that removed it is not named** (R1). The result carries a `[full output: …]` pointer,
but not what was kept, out of what, or by which cap. Fix that first.

Four more proposals are small, each with a measured cost and one site that owns it (R2, R3, R5,
R8). Three come after (R4, R7, R9). One idea is an experiment, not a change (X1). Eight ideas did
not survive the pressure test and are cut in section 3, R6 among them, each with the measurement
that cut it. Section 4 corrects the dogfood findings, including one of revision 1's own
corrections.

| # | proposal | evidence | size | verdict |
|---|---|---|---|---|
| R1 | rustc diagnostics survive the bash reducer, and every cut is named | workspace build: 8349 of 48279 B shown, no `error[E0308]`, no location, the byte cut unnamed | M | do first |
| R2 | a job the model polled is not announced again | `<async_result>` arrived 276.3 s and 109.6 s after the poll returned it; two turns, $0.1386 | S | do |
| R3 | a reader with no fenced material gets a prompt that says so | haiku question-children: 2 of 6 answered a plain JSON request | S | do |
| R5 | the unsourced-number check knows the date it printed | `2026` flagged from the environment date; that turn cost $0.6255 | XS | do |
| R8 | two false sentences in bash's own text | `yes \| head -1` exits 0, not 141; the `&&` notice fires on any `&&` in the command | XS | do |
| R4 | "write the plan now" fires on a failed check, not on any failed command | 4 firings in the corpus, none after a build or test; the event does not carry the command | S | do after its plumbing |
| R7 | `find_models` and `rlm.status` report cost | two prompts say it picks the cheapest; the code returns registry order without cost | S | do after R3 |
| R9 | the edit result names a rustfmt diff in the lines it touched | the check exists and its finding is dropped; unmeasured in the corpus | S | measure first |
| X1 | read's default window | read is 65.9% of tool-result bytes; 15 of 20 large reads were whole files | n/a | experiment |

## 1. Method

**The corpus.** I parsed every top-level session log modified in the 14 days before the pass,
in one kernel pass, and measured each claim there before arguing for it. Figures marked "other
sessions" exclude this one, since this session is the dogfood itself and inflates every tool's
count. The corpus is small and dogfood-heavy, which is the main limit on every frequency below.
A count of zero says "not seen in this sample", not "never happens".

**The probes.** Every behaviour claim in section 2 comes from a command run for this document,
with its output quoted. Every cause is cited `file:line` on main @ e53547c2. The binary that ran
the probes is the session's own, not one built at e53547c2. Where that could matter, the
proposal says so, and its seen-red test decides.

**The review.** A cold reviewer re-derived each cause from the code and modelled the reducer by
hand. Where it disagreed, I re-read the lines it cited and re-ran what I could. This revision
keeps what survived and says where revision 1 was wrong.

## 2. Proposals

### R1. Rustc diagnostics survive the bash reducer, and every cut is named

**Measured.** I used a two-crate workspace (`/tmp/redws`): crate `a` builds first and prints only
warnings, and crate `b` has one E0308 and 120 warnings. Here is the same failing build run two
ways through the real bash tool (a third run, `touch` then rebuild, matched the first row), plus
a single-crate control:

| command | shown | `error[E0308]` shown | `b/src/main.rs:121:36` shown | what the result says about the cut |
|---|---|---|---|---|
| `cd /tmp/redws && cargo build 2>&1` | 8349 of 48279 B | no | no | `[666 lines omitted: 170-835]`, the pointer, `exit code: 101` |
| `cargo build --manifest-path /tmp/redws/Cargo.toml 2>&1; echo EXIT=$?` | 5057 of 48235 B | no | no | the pointer only, and no exit status |
| control: one crate, `cd /tmp/redcrate && cargo build` | 8312 of 24423 B | yes | yes | the error happened to print first |

A red `cargo test` with 500 passing tests and one failure (`/tmp/redtest`) was fine: the panic
and `test result: FAILED. 500 passed; 1 failed` both survived. Tests print their verdict at the
end, and builds do not.

**Causes.** Two causes each lose the error on their own. Two more only make things worse.

1. **The capture cut loses it before any reducer runs.** `drain_capped` keeps the first and last
   15,000 bytes of a stream and writes `[N bytes omitted from the middle]` between them
   (process.rs:161-203; `OUTPUT_CAP` 30,000 at process.rs:12). In row 1's spill, which holds
   every byte, the error starts at byte 23920 of 48279, so it is in neither half.
2. **The red cargo path is a line cap, not a diagnostic filter.** On a non-zero exit,
   `cargo()` returns `cap_lines` (reduce.rs:113-115), the same head/tail cut the generic path
   makes. So even on an uncut capture, an error in the middle of the output is dropped, whatever
   the first word was. The reviewer confirmed this by modelling the reducer: on the full
   1457-line output, error line 726 falls outside both ends.
3. **The byte cut is not named.** Its notice is a line in the middle of the capture, so the line
   cap (row 1) or the green-run prefix filter (row 2) drops it, and the bash tool adds the
   capture's cut note only when the reducer kept no pointer of its own (builtins.rs:792). The
   result keeps the `[full output: …]` pointer, but loses what was kept, out of what, and the cap
   that cut it. The loud-caps rule asks for all three (AGENTS.md:203-206).
4. **The omitted-lines notice understates the cut.** `[666 lines omitted: 170-835]`
   (reduce.rs:265) counts lines of the already-cut capture, while the pointer names the spill.
   In the spill, the result shows lines 1-169 and 1384-1458. So 1214 lines are not shown, not
   666. Lines 836-1383 are missing and no notice names them. On this fixture, reading spill
   lines 170-835 as the notice says does find the error at line 726. A cut whose middle lands
   elsewhere would not.

Revision 1 also blamed routing by the first word (reduce.rs:104-108, reduce.rs:47) and the
masked exit status. Neither removes the error. With exit 101, the cargo and generic paths both
call `cap_lines`. With a masked exit, the green filter (reduce.rs:117-131) keeps `error[` lines
but drops every `-->` location row and the byte-cut notice. Both still matter to the fix, because
a fix keyed on the command or the exit status inherits them.

**Change.**
- Recognise rustc diagnostics by their shape, not by the command or the exit status. An
  `error[`, `error:` or `warning:` header followed within a few lines by a ` --> path:line:col`
  row marks the output as diagnostics, whatever the first word was and whatever bash returned.
- For diagnostics, select instead of capping. Keep every `error` block whole, from its header to
  the blank line that ends it. Show warnings as a count plus the first few, then the summary
  lines. When the capture was cut, select from the spill, which holds every byte.
- Name every cut. After filtering, the result carries the byte cut as one notice row with the
  three facts (kept of total bytes, `OUTPUT_CAP` 30000, the spill path to read). The
  omitted-lines notice counts lines of the file the pointer names.

Revision 1 proposed routing by segment through `yi_permission::safety::parse` (safety.rs:137).
That cannot work: `>` is in its `BAIL` set (safety.rs:135, refused at safety.rs:170), so
`cd /tmp/redws && cargo build 2>&1` comes back `Unparsed` and keeps today's routing. `>` is
there so the permission path never mistakes a redirect for a known argv, and it should stay.
Recognising the output's shape makes routing unnecessary.

**Test, seen red.**
- Run the two-crate fixture through the bash tool under both command shapes and expect
  `b/src/main.rs:121:36` in each result. Today both fail, because the error is gone.
- Expect the omitted-lines notice's range, read in the file the pointer names, to be exactly the
  lines the result does not show. Today it fails: the notice ends at 835, and the shown tail
  starts at spill line 1384. A test asserting only that the range contains the error would pass
  today and proves nothing.
- Expect a row with the byte cut's kept/total in both results. Today neither has one.

**Pressure test.**
- *The model could ask for `--message-format=short`.* Measured on the single crate: 14605 bytes,
  error on line 2. But the model has to know to ask, the 120 warnings still arrive, and every
  build already passes through the reducer. The fix belongs where the cost is paid.
- *Raise `OUTPUT_CAP`.* Any cap has a middle. The error's position depends on crate build order,
  which the model does not control.
- *A structured `check` tool with edit anchors.* The JSON diagnostic row is 41 bytes against 278
  for the error block, so it is tighter. But a new tool's description is paid on every request,
  and tools are 24296 of the 52922 request-budget bytes
  (scripts/guardrails/baselines/request_budget.json). This change keeps the location without new
  surface. Revisit only if, after it lands, sessions still miss locations.
- *Shape detection misfires* on a program that prints rustc-shaped text, such as a test that
  echoes compiler output. Then it shows the error blocks, which is more than needed, not less.

**Size** M. **Verdict: do first.** This is a correctness bug in the build loop, the place where a
dropped line costs a wrong edit.

### R2. A job the model polled is not announced again

**Measured.** This happened twice in this session. A `sleep … ; echo done-long` job went to the
background, and `bash job=5 wait=…` returned `job 5 finished (exit 0) … done-long`. The same
`<async_result job="5">` then arrived 276.3 s and 109.6 s later as a new message. Each copy cost
a turn: $0.1386 for the two, from the log's usage records.

**Cause.** This is read from the code at e53547c2 and the logged timing, not from a test run.
- The completion loop takes each settled job not yet reported, marks it reported
  (jobs.rs:286-298), and queues an `<async_result>` on the follow-up queue (wiring.rs:816-827).
  That queue is "read at a running turn's end; an idle session hears it only after its next
  turn" (wiring.rs:814-815).
- `poll_job` sets the reported flag on entry (builtins.rs:949). It returns a finished job's
  output without asking whether the loop already queued it (builtins.rs:954-959).
- So the flag stops a second queueing, and it stops the loop when the poll comes first. It does
  not stop a poll that comes after the loop queued. Both runs settled before their poll: the
  poll came 24.9 s and 25.3 s after the job started, and the jobs slept 20 s and 15 s.

**Change.** Give each job a "delivered by a poll" bit, set when `poll_job` returns its finished
output. When the follow-up queue is drained, skip an `<async_result job=N>` whose job has that
bit set. The reported flag cannot serve: `take_finished` sets it while queueing, so it would
drop every announcement, including ones no poll read. The runtime already depends on yi-tools,
so the drain can read the registry.

**Test, seen red.** Settle a job, let the completion loop queue it, poll it to finished, then
drain the queue. Expect no message; today there is one. The order matters: a test that polls
first is green today, because the poll's flag makes the loop skip the job.

**Pressure test.** Is the late copy a useful reminder? No: the poll result carried the same
output. The risk is suppressing a result the model never read, and the bit is set only when a
poll returned the finished output.

**Size** S. **Verdict: do.**

### R3. A reader with no fenced material gets a prompt that says so

**Measured.** I spawned question-children with
`rlm.run('Reply with JSON {"ok": true} and nothing else.', deny_write=["."])`. Across this
session: haiku answered with JSON 2 of 6 times (`openrouter/anthropic/claude-haiku-4.5`), and the
default model 3 of 3. In the last batch of four haiku children, one answered and three refused.
Two of those refusals said they saw no material or no question. One said "according to my
instruc…", where the log cuts it off.

**Cause.**
- A child with no role becomes a reader (subagent.rs:628-629), and its system prompt is the
  reader prompt (reader.rs:171-172). That prompt says the material "is in the user message,
  fenced as yi-external blocks with numbered lines" and that the material is data, not
  instructions (reader.md:1-5).
- With no partition named, no fence is sent: `fenced` is None (reader.rs:315-316) and `seed`
  returns early (reader.rs:426-428).
- The user message is `[task from parent]` followed by the prompt (subagent/runs.rs:69-70).

Two refusals show the first reading: the promised material is missing, so there is nothing to
answer. The reviewer reads the third as the second: the task is part of the material, and
material is not instructions. I could not confirm that, because the logged refusal is cut off
after "according to my instruc". Either way, the fix below covers both.

**Change.** When no partition is fenced, use a variant of the reader prompt that says no material
is fenced and the question is the whole task, not data. Review the `[task from parent]` label in
the same change, because it is part of what the child reads.

**Test.** This is a prompt change, so it is judged by the eval harness, sent through the same
path a child's first message takes. Acceptance: at least 19 of 20 haiku children return JSON
after the change. An eval that sends the bare prompt without the `[task from parent]` frame is
not the production message and proves nothing.

**Pressure test.** *Always pass a partition instead.* A bare `rlm.run` is a documented
question-child (orchestrate.md:134), so the bare path has to work.

**Size** S. **Verdict: do.** R7 depends on it, because the cheapest family is the one that
refused.

### R5. The unsourced-number check knows the date it printed

**Measured.** In this session, the check flagged `2026-09-27-tool-dogfood-report.md` for `2026`. The figure was
the date from the host-written environment block. Answering the flag cost one turn with 153062
uncached input tokens, $0.6255.

**Cause.** The check counts as sources only user-attributed text and tool-result text
(coupling.rs:345-420). The environment block is not a stored message. It is a per-request tail
(session/run.rs:505-512), and its date line comes from `date +%Y-%m-%d %H:%M %Z`
(environment.rs:53). So a figure only the host printed can never count as a source.

**Change.** Pass the date, model and tree the host rendered this cycle into the check, which
today receives only the store (coupling.rs:692), and add them to its sources. Do not add the
whole block: its token counts and costs are figures the model should still have to source.

**Test, seen red.** Write a `.md` whose only `2026` is the environment's date, with no tool
result or user message containing it, and expect no flag. Today it is flagged. A fixture where
any tool result already contains the figure passes vacuously, because the check is a substring
match (coupling.rs:416).

**Pressure test.** *The environment block is host text, and host text is never the user's
words* (#970). This does not treat it as the user's words. It lets a figure the host printed
count as printed. The 2026-10-01 unsourced-one-line change already cut this path's cost to one
line, and this removes one more false source.

**Size** XS. **Verdict: do.**

### R8. Two false sentences in bash's own text

**Measured.**
- `yes | head -1; echo "yes|head exit=$?"` printed `exit=0`, and `pipefail` was off. The tool
  description says "`x | head` exits 141" (builtins.rs:691). The writer dies with 141 by
  SIGPIPE, but the pipeline returns head's status, so the sentence is false for the status the
  model sees.
- The notice `[exit N inside a && chain: any segment after the failing one did not run]` fires
  when the command text contains `&&` anywhere (builtins.rs:804). `true && true; false`
  printed it, although nothing was skipped.

**Change.** Fix the description sentence. Emit the chain notice only when the command's last
top-level operator is `&&`, because a list's status is its last command's, and only then did a
later segment not run. Find the operator with a quote-aware scan, not with `safety::parse`,
which discards separators (safety.rs:171) and refuses any command containing `>`.

**Test, seen red.** For `true && true; false`: no chain notice (today there is one). For
`false && echo x`: one notice. For the dogfood probe ending in `> ~/… && echo wrote-home`: one
notice, since that segment did not run.

**Size** XS. **Verdict: do.** The description change also shrinks the request budget slightly.

### R4. "Write the plan now" fires on a failed check, not on any failed command

**Measured.** The `failed_check_after_edit` signal fired 4 times in the corpus, 2 of them outside
this session. In those two, the failing bash command nearest the record was a dogfood probe:
`echo out; echo err 1>&2; exit 7` and `exit 7`. For the two in this session, no failing bash
result sits within seven entries of the record, so I could not attribute them from the log. None
of the four was a build or test command.

**Cause.** Any bash result with a non-zero exit after any edit attaches the orchestrate fragment
with a reminder (orchestrate.rs:264-265, text at orchestrate.rs:171). The signal's name says
"check", but the event cannot tell a check from `pwd`: `Event::ToolResult` carries only the tool
name, the exit status and a file count (ext/mod.rs:48-52). The command is not passed where the
event is built (tools.rs:603). The turn-end signal's incident comment (orchestrate.rs:272-273)
records the same failure in the past tense; that path now attaches without a reminder
(orchestrate.rs:277).

**Change.** Two steps.
1. Have the tools layer classify the command where it builds the event, and add the verdict to
   the event as one field: `check: bool`.
2. Remind only when `check` is true. A command is a check when the first word of a top-level
   segment, found by the quote-aware scan R8 needs, is `cargo` with `test`, `nextest`,
   `clippy`, `build` or `check`; or `just`; or `pytest`, `npm test` or `make`. Any other
   failure attaches the fragment silently, as `files_matched` already does
   (orchestrate.rs:261-262).

Not `safety::parse`: it returns `Unparsed` for `cargo test 2>&1`. Not an exact match against the
gate list (orient.rs:40-47) either: it would miss `just check -q`.

**Test.** Seen red in the second step. With the field present, `{bash, exit: 1, check: false}`
after an edit yields no `Remind`, while `check: true` yields one. Against the field-only commit,
both remind. Before the field exists, the two cases are the same value, so no earlier test can
tell them apart.

**Pressure test.** *A non-check failure might still mean the task outgrew one shot.* Maybe, but
a failed `pwd` does not show it, and the fragment still attaches. Only the instruction aimed at
the model is withheld.

**Size** S. **Verdict: do after its plumbing**, which R8's scan shares.

### R7. `find_models` and `rlm.status` report cost

**Read.** `find_models` filters credentialed models by substring and returns them in registry
order with no cost (models.rs:11-34). Two prompts promise more: "`find_models` picks the
cheapest for readers" (orchestrate.md:163), and readers run "on a cheaper model when
`rlm.find_models` offers one" (doctrine.md:331). `MemberView` carries `tokens` and `idle_s` but
no cost (family.rs:131-139), although every assistant message's usage record carries
`cost.total`.

**Change.** `find_models` returns each entry's cost and keeps its order. `MemberView` gains the
child's summed cost. The two prompt sentences change to call cost one input to the choice.

Revision 1 also sorted by cost. Cut: the cheapest family here was the one that refused (R3), so
cheapest-first would make the failing model the default. The judge tier's cheapest-first sort
(`other_families`, models.rs:62-70) serves a different rule and stays.

**Test.** Expect a cost on each entry, and expect `rlm.status` to report the summed cost of a
child whose usage is known. No test asserts the order.

**Size** S. **Verdict: do after R3.**

### R9. The edit result names a rustfmt diff in the lines it touched

**Read.** After an edit to a `.rs` file, the syntax check runs `rustfmt --check` (syntax.rs:22)
and counts only `error` lines as failures (syntax.rs:111-113). A formatting diff in the edited
lines passes as `syntax: ok`, and `just check` fails on it later through fmt-check
(justfile:34, justfile:58).

**Change.** When rustfmt's diff overlaps the edited lines, the edit result says so in one line
and names `cargo fmt`. It does not reformat. Matching diff hunks to edited lines is more than a
string check.

**Pressure test.** *Noise on files that were already unformatted.* That is why it is scoped to
hunks overlapping the edit. There is nothing to see red until the frequency is known.

**Size** S. **Verdict: measure first, measured, cut for now.** In the 38 local session logs, one
session ran `just check` red, and its failure was `recipe guardrails failed`, not fmt-check.
There is no case to build for. Revisit when CI records show fmt-check as a first red.

### X1. Read's default window: an experiment, not a change

**Measured.** In other sessions, read produced 65.9% of all tool-result bytes, bash 10.9%,
ipython 9.9%, todo and plan together 4.0%. Twenty of 82 reads were over 8 KiB, and they carried
80.8% of read bytes. Fifteen of those twenty asked for no window. No read re-read an unchanged
file.

**Why not a change.** A smaller default trades bytes for turns, and the doctrine says "Prefer
one large read to many small ones" (doctrine.md:142). A default window the model did not ask for
is also a cap, so it would need its own notice (AGENTS.md:214-217). The corpus cannot show
whether those whole-file reads were needed. Run an eval with today's default against "skeleton
plus the first window for files over N KiB unwindowed", scored on task success and total tokens,
before touching it.

## 3. Cut, with the measurement that cut each

The sample is too small to show that any of these never matters, and large enough to show that
none of them is where the cost is today.

- **R6, the plan tool lifting session todos by id.** In the two other sessions that built a
  todo list and then a plan, the plan rows retyped the list rows, with 2 near-duplicate pairs in
  each (for example "Report findings" against "Write findings report"). The cause is real: the
  mirror and the duplicate refusal match labels exactly (mirror.rs:176, todo/mod.rs:461-484).
  But a new plan field moves the tool schema, the tool-surface lock and the request budget, and
  two pairs in two sessions do not pay for that. The doctrine's "Lift the todos" (doctrine.md:128)
  can be done today by dropping the copied rows.
- **A terse mode for todo and plan results.** Together they were 4.0% of result bytes in other
  sessions. They are not where context goes.
- **Aligning the kernel's 65,536-char cap with bash's 8,192-byte floor.** Only one ipython
  result over 8 KiB appeared in other sessions, and it was a probe printing 70,000 characters on
  purpose.
- **A new `check` tool.** Folded into R1, which keeps the location without new request surface.
- **A model-facing undo.** `/undo` (slash.rs:23) and `yi undo` (cli/src/main.rs:1117) exist for
  the user. The checkpoint stages the whole tree with `git add --all` (tools/checkpoint.rs:66), so
  a model-run undo in a shared tree would revert another session's work, which the doctrine
  forbids ("Never revert a change you did not make", doctrine.md:305).
- **A session-diff tool.** In a lane, `git diff` is the session diff.
- **A native grid tool.** Across the whole corpus, 7 of 144 bash calls ran grid, 3 of 12
  `get_context` calls passed a symbol (0 of 8 in other sessions), and 6 reads used `find=`
  (1 in other sessions). That is not demand. Revisit with a larger corpus.
- **`todo set` closing an open plan.** It already says so in its result (todo/tool.rs:393-407).
  All three occurrences were in this session, and each plan it closed had every todo done.

## 4. Corrections to the dogfood findings

- **E2 was right, and revision 1's correction of it was wrong.** The dogfood report said the
  bash result claimed `[exit 1 inside a && chain …]` for a command with no `&&`, or "for a `;`
  chain". Every logged probe command ended `echo y > ~/yidog_probeN && echo wrote-home`. The
  write was refused, so `echo wrote-home` did not run, and the notice was true. Revision 1
  quoted the command without that last segment. With E2 counted as fixed, run 3 had all 15 items
  behaving as asked. An erratum in 2026-09-27-tool-dogfood-report.md says so. R8 still stands: the notice's test
  (builtins.rs:804) is a substring match that fires wrongly on `true && true; false`.
- **The claim that grep's documented page size and its collection cap disagree is wrong.** It
  appeared in my recommendations, not in 2026-09-27-tool-dogfood-report.md. `PAGE_CAP` is 200 and
  `COLLECTION_CAP` is 2,000 (grep.rs:14-15). The run-1 message "beyond the 100 collected
  matches" reported the true total.
- **The duplicate `just check` in `get_context` (2026-09-27-tool-dogfood-report.md:27) is fixed on main.**
  `gates()` dedups by command (orient.rs:421). At 673dd1c6 it did not.

## 5. Order of work

R1 first. Then R2, R8 and R5, which are independent. Then R3, then R7 after it. R4 comes with
the command scan it shares with R8. R9 waits for its measurement, and X1 for an eval slot. Each
change carries its change file, and each test is seen red against main before its fix, per the
testing doctrine.
