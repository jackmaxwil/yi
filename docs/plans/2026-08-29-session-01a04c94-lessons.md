# Session 01a04c94 — what a real run says about Yi's own affordances

```
status:  EVIDENCE 2026-08-29. Not a work plan: the work is seven rows in
         docs/TODOS.md (`A12` `F7` `O10` `P8` `P9` `P10` `P11`), worked in that
         file's order like every other row. This document is why they exist,
         what was measured, and what was rejected — four findings died on
         contact with the tree and the negative space is most of the value.
         `F7` needs a D-row before code; ARCHITECTURE.md held D73 at version
         0.66.0 when this was written and another session in this shared tree
         may have claimed the next number since.
date:    2026-08-29
source:  ~/.yi/sessions/--Users-jackmazac-Development-yi--/1787991167738_01a04c94-12f8-7240-81ce-a860534f459b.jsonl
         — 93 entries, 316 KB, 718 s wall, 25 assistant turns, 30 tool calls,
         parent and both children on openrouter/z-ai/glm-5.3-flash. The run
         digested two PDFs, mapped a 196-file Python codebase under
         ref/agents/strix, and wrote three documents to ref/analysis/.
         · Yi: crates/runtime/src/{affordance.rs,subagent.rs,session.rs,tools.rs,
         ext/orchestrate.rs,ext/mod.rs,prompts/{identity,orchestrate,doctrine}.md},
         crates/context/src/{attribution.rs,policy.rs,convert.rs},
         crates/tui/src/{app.rs,render.rs,status.rs},
         crates/tools/src/{builtins.rs,ipython.rs,hashline/{format.rs,tool.rs,prompt.md}},
         crates/ai/{src/openai.rs,data/openrouter.json},
         python/yi_runtime/src/rlm/__init__.py,
         scripts/guardrails/{check_guardrails.sh,check_request_budget.py},
         docs/YI_DESIGN.md §8.14 U16
note:    the session is Yi driving itself; there is no external codebase to
         port from and no Appendix A question. Every row below is a fix to Yi's
         own text or accounting, not an adoption.
```

The run was good at the thing that is hard and bad at the thing that is
mechanical. Of 143 unique `file:line` citations its delegate produced, 140
resolve in range and a 16-point semantic spot-check was 16 of 16 exact. Of 70
systems it named from a survey PDF, 70 appear in the source text, every
quantitative claim matches, one figure was read correctly off a bar chart in
`pdftotext -layout` output, and a negative claim was true. The synthesis has
0.0 % 8-gram overlap with its two sources. That is on a cheap flash model.

It then spent 43 % of its wall clock on Yi's own instructions being wrong.

## 1. Thesis

Praxist's thesis was *a value that is unknown is being recorded as a value that
is known*. This session's is narrower and closer to home.

**Yi's affordances describe an API Yi does not have.**

- The always-on system prompt shows `h = rlm.run(...)` without `await`, so the
  child never starts (`P8`).
- The spawn affordance names a transcript path nothing writes to (`P9`).
- The status line's `$cost` segment is one turn's cost rendered where a session
  total belongs (`A12`).

Each is text that was true of an intended design and is false of the built one.
None of them is a hard failure: the run finished, the answer was right, and
nothing errored except downstream of the first. That is what makes them
expensive — an affordance that is merely wrong gets followed.

## 2. What survived contact with the code

### The `rlm.run` trap is a prompt typo, not an API defect (`P8`, `O10`)

`crates/runtime/src/prompts/identity.md:16`, which every turn pays for:

```
      h = rlm.run("Port crates/foo to the new API. Report the files changed.")
      done = await rlm.wait(120)
      r = await h.result()
```

`rlm.run` is `async def` (`python/yi_runtime/src/rlm/__init__.py:183`). Line 16
produces an un-awaited coroutine and starts nothing. The example also teaches a
false model — that `run` returns a handle synchronously and `wait` is the
blocking step — and the session's seq-18 thinking repeats it back:

> The handle is a coroutine — I can await it in ipython. Actually, await will
> block until the child finishes; the children may take a while.

Awaiting the spawn blocks until *admission*, not completion, so the deferral
bought nothing and cost everything. `crates/runtime/src/prompts/orchestrate.md:63`
carries the same defect and line 66 drops an `await` on `merge_worktree`. Every
other `rlm.*` and `goal.*` example across the prompts and the bundled skills
awaits correctly, so these three lines are typos rather than a convention.

Measured: the spawn call at t=95.7 s, the children not admitted until t=177.5 s
and t=184.3 s. Nine turns and 57 s of API archaeology across seq 26-42. All
three errored tool results in the run sit in that stretch.

The correction that already exists and cannot be reached: `affordance::spawned`
(`crates/runtime/src/affordance.rs:9`) builds the right line, `subagent.rs:541`
puts it in the reply's `next` field, `_spawn_handle_from_payload` reads it and
`RLMSpawnHandle.__repr__` prints it. It materialises only on the handle, and the
handle materialises only after the await it exists to teach.

`P8` fixes the three lines and adds one predicate beside `grid_note`
(`crates/runtime/src/tools.rs:60-70`) so an `ipython` result carrying
`<coroutine object run at` gets the hint appended through the existing
`affordance::append` seam. That catches every caller, in the place the agent
reads, with no Python change — so no runtime-identity-hash move and no venv
rebuild. `O10` is the gate that stops the typo returning: collect `async def`
names from the runtime package and fail any example call not preceded by `await`.

Rewriting `rlm.run` as a plain `def` that schedules internally was considered and
rejected. It moves the runtime identity hash, and it has to keep
`await rlm.run(...)` working for every existing correct caller, which means
returning an awaitable handle. That is real machinery for a case one predicate
already catches.

### The spawn affordance promises a transcript nothing writes (`P9`, `F7`)

`affordance::spawned` ends with *"its transcript is `<session_dir>/*.jsonl`"*.
Nothing attaches a store to a child: `attach_store` (`session.rs:394`) is called
only from the two rewind paths and from tests, and a child's `session_dir` is
handed to its kernel service as its own `rlm_dir` (`subagent.rs:877`, `1107`),
never to a session repo. `~/.yi/sessions/rlm-77036/sub-dd02ff6f/` and
`sub-f243f3d3/` were empty during the run and are empty now; no file under
`~/.yi/sessions` mentions either child id except the parent transcript.

The agent followed the hint and burned seq 36 and seq 38 listing empty
directories. Its seq-40 thinking: *"The session dirs are empty — the children
didn't write outputs there."*

Split deliberately. `P9` deletes the clause, because the measured cost is the
promise and the promise is one line. `F7` writes the transcript, because the
capability is real but carries three questions code cannot settle on its own:
`JsonlRepo::new(root, cwd)` writes `<root>/--<encoded-cwd>--/<ts>_<id>.jsonl`
(`crates/session/src/jsonl.rs:25-37,140-151`), so the promised flat layout is a
choice; `subagent.rs:464,474` already removes a child dir on spawn failure, so
whether a transcript outlives its child is a policy; and a family that costs
nothing on disk today would stop doing so. That is a D-row.

What is not missing: live observability. `F1` puts `ChildUpdate` on the parent
bus, `/agents` shows per-child status, tokens and tool calls, and a failing
child's error reaches the parent verbatim in the completion notice
(`subagent.rs:614-621`). The gap is post-mortem evidence, which is second-order.

### The status line shows one turn's cost where a session total belongs (`A12`)

`crates/tui/src/render.rs:94` assigns `app.cost_total` from
`AgentSession::last_usage()`. That value is *replaced* at every assistant
`MessageEnd` (`session.rs:868-872`), so it is one turn. `attribute_to_shared`
(`session.rs:1165-1173`) folds child usage into the in-memory last assistant
message and appends a lane record but never touches `shared.last_usage`, so
child cost never reaches the HUD at all. `status.rs:86-88` renders it as a bare
`$X.XX` beside `N% of 1M`, which reads as the session's.

For this session it renders `$0.00`. The run cost `$0.0434`: `$0.0190` of parent
own usage across 25 turns, `$0.0244` of child usage across 32.

The fix is three lines inside `yi-tui`, accumulating in `App::reduce_message_end`
(`app.rs:617`). Children come free: `reduce_child` (`app.rs:677`) routes their
events through the same reducer and `sync_children` (`app.rs:1061-1070`)
subscribes to every child unconditionally rather than on focus. No double-count,
because the `attribute_to_shared` fold is never re-emitted as an event.

Two things it deliberately does not do. No `AgentSession::session_cost()`:
`session.rs` is 1,193 lines against the 1,200 ceiling, and a module at its
ceiling splits at a seam rather than by line count. No `cost` field on
`ChildUpdate`: that is a wire shape needing a fixture and a `schemas.lock` diff,
and the event path makes it unnecessary.

`YI_DESIGN.md:1054` (U16) specifies `$cost` without saying turn or session, and
the port source is OMP `segments.ts:114-511`, a read-only-reference span where
the cost segment is the session's. Read that span before implementing; if OMP
shows per-turn, this becomes a decision and needs a D-row first.

### Two prefilter features are blind to what the run actually did (`P10`)

`enumerations()` (`ext/orchestrate.rs:56-67`) counts `- ` lines and numeric
lines. The session's prompt had 18 `* ` bullets and 0 `- ` bullets and scored
`enums = 0`. `*` and `+` are equally valid CommonMark bullets; this is a parity
bug, not a threshold change.

`files_matched` (`tools.rs:26-38`) returns 0 for anything but `grep` and `glob`,
so a session that explores through bash is invisible to the `files_matched > 5`
escalation. This session made zero `grep`/`glob` calls, and it was right not to:
`identity.md` itself instructs `rg` through bash for regex.

Neither changed this run's outcome, which is the honest framing the row carries.
The prefilter scored 9 against a threshold of 4 and attached `orchestrate.md`
anyway. The recommendation on `files_matched` is to state the scope rather than
widen it — parsing bash output for path-shaped hits is a heuristic on a
heuristic, and `P4` is where this settles with telemetry rather than a guess.

### Context round-tripping, with the caveat that governs it (`P11`)

At seq 78 the agent ran `print(survey_res['text'])`, putting 26,490 chars into
context; seq 80's input is 7,347 tokens. The file was written at seq 86 straight
from the kernel variable, never from context — that 7,347 is provably wasted. At
seq 80-82 it printed 18,747 chars in and re-typed 18,832 chars back out through
`write` (seq 82 output 6,725 tokens) for what `open(path,'w').write(var)` does
for free, the one-liner it found at seq 86, one call too late. Against 94,047
non-cached parent tokens that is roughly 20 %.

The caveat: `orchestrate.md`'s "Compute in program space" paragraph already says
exactly this, and the prefilter attached it. More prose will not fix a paragraph
that was present and ignored. `P11` is therefore mechanical — the `grid_note`
seam again, on the print rather than on the write so it lands before the
expensive mistake — and the row says outright that if a repeat run does not move,
it is one predicate to delete.

## 3. Rejected, with reasons

**Parent usage is never persisted.** False, and it was the largest claim in the
first pass. Every assistant message on disk carries a full `usage` object: 25
messages, in 75,597 / out 18,450 / cacheRead 584,448, `cost.total` summing to
`$0.0190`. The first pass counted only `type: "usage"` lane records, which exist
specifically for child attribution (`attribution.rs:5`, design P14). A Yi session
is fully costable from its own transcript. What is broken is the HUD, which is
`A12` and needs no schema field. `M6` remains the right home for the separate
question of usage that is unknown rather than zero.

**`read`'s `[path#812B]` banner reads as a byte count.** It does, to a human
reading a transcript. It is a `FileTag(u16)` rendered `{:04X}`
(`hashline/format.rs:23`), the snapshot tag `FileTag::parse` requires to be
exactly four hex characters and `patcher.rs:225,505` reads on every edit. The
`read` description already says *"Output starts with a [path#TAG] snapshot
header … use both to anchor edits"* and `hashline/prompt.md:4` says *"`TAG`:
4-hex snapshot"*. The agent never misused it. Changing the format moves the edit
protocol, its parser, `HL_FILE_HASH_EXAMPLES` and the OMP-derived fixtures, to
correct a misreading by a reader who is not the consumer. No row, and no
additional documentation either.

**The dedicated search tools went unused.** They did, and correctly.
`identity.md` states *"`grep` matches a literal substring … For regex,
multiline, or type-filtered searches, run `rg` through bash"*. Every bash grep in
the session used alternation. Yi's `grep` could not have served one of them. The
residual risk is the agent's own `head -N` truncation, which is its pipe and not
a tool contract; `C9` already covers the half Yi owns.

**Compaction never fired on 45 KB of tool output.** Correct behaviour.
`should_compact` triggers above `window − reserve_tokens`
(`context/src/policy.rs:14,24`), and `z-ai/glm-5.3-flash` carries
`contextWindow` 1,048,576, so the trigger is 1,032,192 tokens. Peak context this
session was 51,043. Nothing to tune, and the first pass had no window figure when
it called this a defect.

**Something in the harness rewards emitting a call over emitting none.** No. The
empty `ipython` cell at seq 26 was a slip — seq 28's thinking reads *"Oops, empty
call."* — and the `cp /dev/null` at seq 84 is model padding of a parallel slot.
Fixing `P8` removes the motive for the poll that produced the empty cell, which
is the only part Yi can act on.

**The plan skill is not discoverable.** Partly false. Doctrine carries "Plan when
it pays", `orchestrate.md` was attached and carries the full task / acceptance /
check contract, and `/plan` and `/goal` are in `SLASH_COMMANDS` (`app.rs:75`).
The agent wrote a plan in its thinking at seq 9 and followed it. No row.

**A third payment on the written bytes.** The first reading had the `write`
tool's `details.patch` echoing 18,832 bytes back into context. It does not:
`push_tool_results` destructures `{ tool_call_id, content, .. }`
(`crates/ai/src/openai.rs:197-201`) and drops `details` entirely. Seq 84's input
is seq 82's own output billing forward, which is ordinary. Corrected here because
the wrong version made `P11` look larger than it is.

**Warn when a write lands on a gitignored path.** The session put 64 KB of real
work product in `ref/analysis/`, which `git check-ignore -v` resolves to
`.gitignore:2:/ref/`. `crate::ignore::Ignore` exists and `walk_files` already
uses it (`builtins.rs:106`), so the affordance is about fifteen lines on the seam
`P8` and `P11` already use. Not opened as a row: most agent writes to ignored
paths are legitimate (`target/`, `.yi/`, scratch), the noise floor is unmeasured,
and `ref/` being ignored is a fact of this repository rather than a general one.
It becomes a row the first time a second run loses work this way.

## 4. One measurement to repeat, not a fix

Turn seq 30 reports `input 24,603` with `cacheRead 0`, against a steady
`cacheRead` of 21,000-25,000 and `input` under 1,000 on the turns either side —
four times the neighbouring cost. It sits between the two child admissions, and
child 1's first request (`input 5,060`, `cacheRead 0`) lands in the same window,
on the same provider, model and account.

The obvious hypothesis is that a child's uncached request evicted the parent's
prefix. One observation cannot establish that, and the alternative — an ordinary
provider-side eviction — is not excluded. It is recorded here rather than acted
on. `J5` already says the request-prefix ratchet measures bytes and not billed
tokens; if the eviction is real it is the same warning from the other side, and
the instrument that would settle it is `P4`'s route telemetry plus per-turn
`cacheRead` from the session file, which needs no new field.

## 5. What not to trade away

The delegated analysis is the part of this run worth protecting, and every row
above is either a wrong instruction removed or an accounting bug fixed. None
changes what the agent is told to verify, what a child brief must contain, or how
a claim is evidenced.

The failure mode to watch for in `P11` specifically: an affordance that nudges
toward printing less could, badly worded, nudge toward reading less. Its text
names the mechanism (`write files with open(path,'w').write(var)`) rather than
the goal (`use fewer tokens`), because the second phrasing is the one that would
buy speed with evidence.
