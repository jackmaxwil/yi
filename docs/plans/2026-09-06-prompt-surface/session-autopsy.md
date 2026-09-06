# Session autopsy: "analyze the yi repo comprehensively. rate it out of 10"

Session: `~/.yi/sessions/--Users-jackmazac-Development-yi--/1788679797854_01a0759f-….jsonl`
Model: z-ai/glm-5.3-flash, medium. 12 tool calls, 6m27s wall, 199K input tokens, $0.012.

## What happened, by the numbers

| phase | calls | tool wall time | share |
|---|---|---|---|
| orientation (ls, get_context, ARCHITECTURE head, justfile, grep counts) | 5 | 16.5 s | 6.5 % |
| clippy + nextest + retries | 7 | 235.6 s | 93.5 % |

Total tool time 252 s. Of the 12 calls, 8 (entries 27-45) were gate runs and their retries. Zero `read` calls on a source file, zero on YI_DESIGN.md, CHANGELOG.md, or `git log`. The "comprehensive analysis" never opened a `.rs` file.

Token cost: 209K prompt tokens for a 20K context. Provider cache hit on 4 of 13 calls (cacheWrite always 0, so z-ai's cache is opportunistic); every test turn re-paid the full ~18K prefix.

## Why running tests was the wrong move

**1. Wrong evidence class for the task.** "Rate the codebase" is a reading task. Its evidence is structure, design decisions, code, docs, history. Whether the suite is green *right now* is a property of the working tree at one instant, and the repo already publishes it: `just check` is the merge gate, the forge CI is required, CHANGELOG rows carry the gate. Re-measuring a published signal is redundant at best. The model's own first instinct was right (entry 20 thinking: "Tests might be long... Let's do a lightweight probe") and it overrode itself two turns later ("Let me run the actual gate to verify quality") with no principle in the prompt to stop it.

**2. The measurement contaminated the result.** Tests failed because the agent runs inside Yi's own sandbox (macOS Seatbelt): `bind: Os { code: 1, kind: PermissionDenied }` at daemon.rs:740. That is a fact about the *measuring instrument*, not the codebase. The model diagnosed this correctly (entry 47 thinking: "failures are environmental") and then **penalized the codebase anyway**: "test portability 6.5" and "It loses the last 1.5 points to the test portability". The most expensive 4 minutes of the session produced the one finding that was wrong, and that finding set the score.

**3. It changed subjects mid-analysis.** Entry 23: `cd ~/.yi/lanes/897d6e9162485667/1` — the model moved from the cwd to a lane worktree it never mentioned. Same commit by luck. Nothing in the prompt says what a lane is or that it is the sandboxed cwd; the model treated it as a random path in the environment block.

**4. Doctrine bug, not model bug.** doctrine.md "Done is a measurement: run the relevant check before claiming finished" has no scope condition. A weak model generalizes "run the check" from *changes I made* to *any claim I make*. "Claims stay checkable" (identity.md) reinforced it: clippy got run twice to mark it "(verified)". The rules that exist are correct for edits and wrong for assessments; the prompt never distinguishes task classes.

**5. Tool syntax flailing.** 4 of the 8 gate calls were wasted: `--quiet` is not a nextest flag; the filter `sandbox` matched 0 of 105 tests twice (integration test binaries need `-E 'binary(sandbox)'`); `grep -A12` on nextest output returned nothing. The justfile the model had already read (entry 21) contains the exact test recipe. It read the recipe and then typed its own.

**6. Self-inflicted truncation, blamed on the environment.**
- Entry 13: `find . -name "*.rs" | head -40 && echo --- && wc -l …` — head closed the pipe, the `&&` chain stopped, wc and `cat Cargo.toml` never ran. Model did not notice.
- Entry 17: output was *reduced* (rawBytes 8715 → outBytes 7377, bridge=compound) with a `[full output: ~/.yi/tool-output/30f4195b.txt]` pointer. The `119967 total` line count and the `ls scripts/guardrails` listing were in the cut. The model never opened the pointer file, then later wrote "~133k lines" and "25+ Python checks" anyway.
- Entry 21: the model set `max_output_lines: 60` itself; clippy `--quiet` printed nothing because it was clean; the model read empty-as-truncated ("the previous turn's tool output got truncated by the environment") and re-ran clippy for 11 s.

**7. The counts it did report are wrong in the direction of confidence.**
- "only 7 `.expect()`" in production: all 7 are inside `#[cfg(test)]` modules (daemon.rs:740-744, console diffs.rs:266-317). The grep excluded the attribute line, not the module body. Production count is 0.
- "3 `unsafe` across all crates": one is a test (`tui_e2e.rs:321`), one is the English word in a comment (`schedule/mod.rs:467`), one is in `vendor/rtk`. Crate count is 0; every crate carries `forbid(unsafe_code)`.
- "1,054 tests" and "950 tests" are the same measurement taken twice (grep `#[test]` vs nextest's count).
- "15 crates" is right. Yi's own CLAUDE.md/AGENTS.md says "Thirteen crates" (.ruler/020-architecture.md:3): stale instruction source, worth fixing.

**8. Router mismatch.** `ext_record route: one_shot, score -3` for a 10-word prompt. Twelve tool calls later, `orchestrate_attached (signal: tool_calls_per_turn)` fired — after the turn ended. The deterministic router priced "analyze comprehensively" as a one-shot; the corrective fragment arrived post hoc.

## What a good run looks like (same budget)

~10 calls, ~1 minute, all reads: ARCHITECTURE.md header + decision log tail, YI_DESIGN.md §1, `git log --oneline -30`, CHANGELOG tail, three source files chosen from the orientation packet's skeletons (one boundary crate, one hot module, one test), the justfile, `scripts/guardrails/` listing, the CI status line. Gate status is *quoted* from the repo's own record, not re-run. Rating dimensions are then design, correctness discipline, docs, maintainability, scope, with evidence being things it read.

## Mechanisms that would have prevented it

1. **Task-class gate in doctrine**: "Run a check only to verify a change you made. For a question or assessment, quote the repository's own gate record; never run the suite to learn what CI already says."
2. **Sandbox self-knowledge in the environment block**: "You run inside a Seatbelt sandbox in lane `<path>`: Unix socket bind, network, and writes outside the lane are denied. A test that fails with PermissionDenied here is not evidence about the code."
3. **Reduced-output contract**: when reduce cuts bytes, the pointer line must say *what* was cut (`[reduced 1338 B: lines 100-140 of wc, ls]`) and the model must be told that a `[full output: …]` pointer is a file it can `read`.
4. **Chain-stop detection**: a compound `&&` command whose later segments produced nothing gets a host note ("segment 3 of 5 did not run: pipeline exit 141").
5. **Repo test recipe as a skill or environment fact**: `just test` / nextest filter syntax, one line, so the model stops guessing flags.
6. **Count hygiene rule**: "A grep count is not a finding. Before reporting a count, read the matches."
7. **Router**: "analyze / rate / review / explain" verbs route to a read-only assessment profile that denies mutation and gate runs by default.
