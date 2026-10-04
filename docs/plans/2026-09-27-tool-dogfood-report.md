# Yi tool dogfood report

Session of 2026-09-27 (`date +%F` printed `2026-09-27`). Every tool was used blind first, on scratch files in `/tmp/yidog` and `scratch_dog/`. Source was read only afterwards, to confirm the two defects below.

## Verdict

The tools are good to use. Error messages are the strongest part: nearly every refusal says what was wrong and gives the exact form to use instead. Stale-tag rebasing and cross-call registers make chained edits reliable. I found two real defects (absolute globs, silent CRLF flattening in mixed files), one broken kernel helper (`rlm.put`), and several ergonomics problems. The worst of those is `[grid check]` noise on every edit.

## Defects

| # | Tool | Reproduction | Observed | Cause |
|---|------|--------------|----------|-------|
| 1 | read | `read path="/tmp/yidog/*.rs"` (a.rs exists) | `no file matches "/tmp/yidog/*.rs"` | `hashline/tool.rs:391-395`: the walk is rooted at `context.cwd` and matches the cwd-relative path, so an absolute glob, or one outside the cwd, can never match. A plain absolute file path does work. |
| 2 | edit | File with an LF first line and CRLF later lines; edit line 3 | Line 4's `\r\n` became `\n` (checked with `od -c`). The edit touched a different line. | `hashline/normalize.rs:7-13` picks CRLF only when the *first* ending is CRLF. `normalize_to_lf` then removes every `\r`, and restore does not put them back. This is a silent byte change outside the edited range. |
| 3 | ipython | `rlm.put("probe", {...})` | `PermissionError: [Errno 1] Operation not permitted: '~/.yi/sessions/family'` | The kernel's sandbox refuses the blackboard directory. So the documented `put`/`get` map-reduce pattern does not work from the main kernel in auto mode. |
| 4 | rlm.run | `model="anthropic/claude-haiku-4-5"`, a model `find_models` listed | Child failed in 0.5 s: `HTTP 401 ... API key is invalid.` | `find_models` lists models the configured credentials cannot call. The default model worked. |
| 5 | todo | `append` two items with the same label | Both accepted, as t13 and t14. A later `rm label="scratch probe item"` removed both at once (the count went from 14 to 12). | The description says "Labels are verbatim and unique". Uniqueness is not enforced, so an op by label can hit several items. |

## Ergonomics findings

- The `[grid check]` block is noise. Every edit, including edits to throwaway `.py`/`.md`/`.rs` files outside any crate, printed the same unrelated `drift ... yi_tools.tool.Tool 8bdc→48c3` lines. The signal-to-noise ratio trains the reader to skip it, which defeats a gate.
- The bash sandbox hint names the wrong command. `yes | head -5; echo x > /tmp/...; echo y > ~/yidog_probe` failed on the home-directory write, but the `next:` line said "`yes` now needs permission". It also says "exit 1 inside a && chain" for a chain with no `&&`.
- The bash `wait` parameter did not become a job. `sleep 20` with `wait=5` just blocked and returned `done-long` inline. The documented "a longer command becomes a job" did not happen. This is harmless, but it means I could not exercise the job path.
- bash has a network contradiction. `curl https://example.com` printed `net-ok`, but the sandbox note says "no network". Either this call was not contained, or the note is wrong. The user cannot tell which from the output.
- Overlapping hunks are accepted silently. `PUT 2*:` (the `if` block, lines 2-3) plus `PUT 3.=3:` in one call produced duplicated `return` lines with no warning.
- An edit is allowed on a file never shown this session. The doctrine says undisplayed hunks are rejected. Yet `scratch_dog/k.py`, written by the kernel and never read, took `PUT >2 @keep` with no tagless-section complaint.
- `get_context symbol=HashlineEditTool` reported `grid: nothing is called HashlineEditTool — closest:` (with an empty list). It still ranked the defining file first. `grid resolve HashlineEditTool` from bash resolves it fine, so the packet passes the name to `grid scope` in a form grid rejects. The gate list also shows `just check` twice, because `justfile` and `Justfile` both exist on this case-insensitive FS.
- Large ipython output is truncated at 64 KiB with no `[full output: path]` pointer, unlike bash.
- `find=` searches refs across the whole repo. `find=Sub` in a 15-line scratch markdown file returned 20 refs from `crates/`, which has nothing to do with that file.
- Plan `done` on a pending todo is refused ("start it first"). It is correct, but it costs an extra call per todo that has no contract. `set` with `- [x]` is the stated workaround.

## What worked well

- read: ranges with a gap marker (`[lines 4-4997 not shown]`), `offset` continuation hints, the binary refusal, the offset-past-EOF message, relative globs and brace sets (`*.{rs,md}`), directory skeletons, and `~` and `..` paths.
- grep: literal and regex modes, arrays of patterns, `count`, `def`, a `replace` preview as a unified diff with a one-step `apply`, a clear regex parse error, and the refusals for include+type and a missing pattern. `block` falls back to context and says so.
- edit: tag mismatches show the current tag and a marked excerpt. Stale tags rebase with a warning when the cited lines are unchanged. `PUT N:` shorthand works. Refusals for a missing `+`, a reversed range, out-of-range lines, and an invalid tag all point at the correct form. Named registers persist across calls and across files, and an empty register lists the ones that exist. `MV` and `REM` compose with edits in the same patch. The Markdown heading block op replaced exactly one section. The protected-path write was denied.
- bash: exit codes, chain-stopped notes, stderr separated, `timeout_secs` enforced (`exit code: -1`), output cut with a full-output path, and a fresh cwd per call.
- ipython: state persists, errors keep the kernel, `%%bash` runs, `rlm.bash` handles poll and wait, and the un-awaited coroutine hint is exact.
- rlm.run reader (default model): schema-valid JSON in 17.9 s using 107,851 tokens (from `rlm.status()`). It declined to guess `bin.dat`'s line count rather than invent one.
- todo and plan: unknown ops and missing evidence are refused with examples. The plan contract ran `test -f` and recorded `verified_done`. Dependency ordering and the "user message not cited" trace both work.

## Suggested fixes, in priority order

1. Fix CRLF detection for mixed files. Preserve each line's own ending, or refuse to normalize outside the edited range.
2. Make glob reads accept absolute patterns. Root the walk at the pattern's literal prefix.
3. Scope `[grid check]` to symbols the edit touched, and drop it for files outside the charted crates.
4. Fix the bash sandbox `next:` attribution to name the command that failed. Stop claiming a `&&` chain when there is none.
5. Enforce todo label uniqueness, or remove the claim from the description.
6. Make `rlm.put` writable from the kernel, or document that it needs a child.
7. Filter `find_models` to models the configured credentials can call.
8. Reject or warn on overlapping hunks in one edit call.

## Re-run on the updated build

Run on 2026-09-29, against the build merged as `673dd1c6` (`git log --oneline -3`). I repeated each reproduction from the first run. The kernel tally of the 15 items below gave `Counter({'fixed': 8, 'not fixed': 6, 'partly fixed': 1})`.

| Item | Reproduction | Result now | Status |
|------|--------------|------------|--------|
| D1 absolute glob | `read "/tmp/yidog2/*.rs"` | `no file matches "/tmp/yidog2/*.rs"` | not fixed |
| D2 CRLF in mixed file | edit line 2 of `odd.txt` (LF first line, CRLF later) | `od -c` still shows `\r \n` after `em dash` and `crlf line`. The untouched LF line in `mixed2.txt` stayed LF. | fixed |
| D3 `rlm.put` | `rlm.put("probe", {...})` | `put ok {'a': [1, 2, 3]}` | fixed |
| D4 `find_models` | `rlm.run` on `openrouter/anthropic/claude-haiku-4.5` from `find_models("haiku")` | `{'ok': True}` in 1.2 s | fixed |
| D5 duplicate labels | `todo append ["dup probe", "dup probe"]` | `todo "dup probe" is already in the list; labels are unique` | fixed |
| E1 grid check noise | edit `scratch_dog/a.rs` | the same `drift proven ... yi_tools.tool.Tool 8bdc→48c3` block | not fixed |
| E2 sandbox hint | `echo a \| head -1; ...; echo y > ~/yidog_probe2` | `next:` now names the refused path, but the result still says `[exit 1 inside a && chain ...]` for a `;` chain | partly fixed |
| E3 wait becomes a job | `sleep 20; echo done-long` with `wait=5` | `[still running after 5s: now job 5 ...]`, then `job 5 finished (exit 0)` | fixed |
| E4 network vs sandbox note | `curl -sS -m 5 https://example.com` | `net-ok`, while the sandbox refusal text still says "has no network" | not fixed |
| E5 overlapping hunks | `PUT 2*:` plus `PUT 3.=3:` in one call | `anchor line 3 is already targeted by another hunk on line 1` | fixed |
| E6 edit on a never-shown file | edit a kernel-written `k2.py` | `No version of scratch_dog/k2.py is on record for this session`, with the current tag and lines offered | fixed |
| E7 `get_context` symbol | `get_context symbol=HashlineEditTool` | `grid: nothing is called HashlineEditTool — closest:`, and `just check` still listed twice | not fixed |
| E8 ipython truncation | `print("A"*70000)` | output ends with `[full output: ...]` | fixed |
| E9 `find=` ref scope | `read scratch_dog/doc.md find=Sub` | `[refs: 20 of 31 for Sub]` from `crates/` | not fixed |
| E10 plan done on pending | `plan done` on a pending todo | `done is illegal ... in state pending; start it first` | not fixed |

Nothing that worked in the first run broke. I re-checked relative brace globs, the grep replace preview, the regex parse error, a register moved across files, the reversed-range refusal, the `/etc` write denial and `timeout_secs`. All of them gave the same results as before.

The remaining priorities, in order: D1 (absolute globs), E1 (grid check noise), then the `&&` wording in E2 and the network note in E4. E10 may be intended behaviour. If so, the report should drop it rather than track it.

## Run 3

Run on 2026-10-01 (`date +%F`), against the same `673dd1c6` checkout (`git log --oneline -3`). The build of Yi running the tools had changed since run 2. I repeated every reproduction. The kernel tally gave `Counter({'still fixed': 8, 'fixed': 6, 'partly fixed': 1})`, so 14 of the 15 items now behave as asked.

| Item | Reproduction | Result now | Status |
|------|--------------|------------|--------|
| D1 absolute glob | `read "/tmp/yidog3/*.rs"` | `[1 files match /tmp/yidog3/*.rs]` and the file shown | fixed |
| D2 CRLF in mixed file | edit line 2 of `odd.txt` | `od -c` shows `\r \n` kept on the untouched CRLF lines | still fixed |
| D3 `rlm.put` | `rlm.put("probe3", {"run": 3})` | `put ok {'run': 3}` | still fixed |
| D4 `find_models` | `rlm.run` on `openrouter/anthropic/claude-haiku-4.5` | the model is callable (2210 tokens, state `finished`). It replied in prose ("I'm designed to answer questions based on material provided in yi-external blocks"), and the schema check refused that. The default model returned `{'ok': True}` in 1.7 s. | still fixed |
| D5 duplicate labels | `todo append ["dup3", "dup3"]` | `todo "dup3" is already in the list; labels are unique` | still fixed |
| E1 grid check noise | edit `scratch_dog/a.rs` | `[grid check: clean]` plus one line: `14 suspects outside the edited files — bash: grid check --quick` | fixed |
| E2 sandbox hint | `echo a \| head -1; ...; echo y > ~/yidog_probe3` | `next:` names the refused path. The result still says `[exit 1 inside a && chain ...]` for a `;` chain. | partly fixed |
| E3 wait becomes a job | `sleep 15; echo done-long` with `wait=5` | `now job 5`, then `job 5 finished (exit 0)` | still fixed |
| E4 network vs sandbox note | `curl -sS -m 5 https://example.com` | `net-ok` plus `sandbox: allowed by user; approving runs this one call outside the sandbox (network)`. The refusal text now reads "no network beyond 127.0.0.1 and ::1". | fixed |
| E5 overlapping hunks | `PUT 2*:` plus `PUT 3.=3:` | `anchor line 3 is already targeted by another hunk on line 1` | still fixed |
| E6 edit on a never-shown file | edit a kernel-written `k3.py` | `No version of scratch_dog/k3.py is on record for this session`, with tag `#16B4` offered | still fixed |
| E7 `get_context` symbol | `get_context symbol=HashlineEditTool` | the symbol neighborhood resolves `crates/tools/src/hashline/tool.rs:759` (`PARTIAL - 5 of 6 layers`). `just check` is listed once, but only `justfile` exists now, so the duplicate case was not re-tested. | fixed |
| E8 ipython truncation | `print("A"*70000)` | output ends with `[full output: ...]` | still fixed |
| E9 `find=` ref scope | `read scratch_dog/doc.md find=Sub` | the block at lines 9-12 and no repo-wide refs | fixed |
| E10 plan done on pending | `plan done` on a pending todo | done, with `note: it was pending, so it was started first` | fixed |

Nothing regressed. I re-checked relative brace globs, the grep replace preview, the regex parse error, a register moved across files, the reversed-range refusal, the `/etc` write denial, `timeout_secs` and the missing-file read.

One item is left: the `&&` chain wording in E2. Run 3 also turned up one new thing to watch. The haiku child answered a plain JSON request by saying it only works on yi-external material, which suggests its brief frames the prompt as data rather than as the task.

## Erratum (2026-10-02)

E2 was not a defect. Each probe command ended `echo y > ~/yidog_probeN && echo wrote-home`. The sandbox refused the write, so `echo wrote-home` did not run, and `[exit 1 inside a && chain: any segment after the failing one did not run]` was true. The rows above that call it wrong, and run 3's "partly fixed", should read "fixed". So all 15 items in run 3 behaved as asked. The notice does fire wrongly on other commands, `true && true; false` among them, because it tests for the substring `&&` (builtins.rs:804). That is a separate defect, proposed as R8 in `docs/plans/2026-10-02-tool-ergonomics-proposals.md`.
