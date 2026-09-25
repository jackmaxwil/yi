# Kernel defect fixes on the Jupyter kernel

```
status:  planned 2026-09-24. Ten small PRs against today's ipykernel/zeromq kernel. S1 (the
         Yi-owned kernel swap) and S2/S3 (per-cell history, handoff) of the kernel-target plan
         stay deferred.
tree:    0.301.0 at 902d1529; last decision row D237. Open PR #506 claims 0.302.0 and D238, so
         this series starts at 0.303.0 and D239. Re-read the header, the last D-row and every
         open PR's claims before writing either.
issues:  #507-#516, one per PR below in order, milestone "Runtime, kernel and prompts".
branch:  claude/kernel-defect-fixes (worktree ~/Development/yi-kernel-defects) carries this
         plan and PR 1. Every later PR branches from origin/main in its own worktree under
         ~/Development (never /private/tmp: seatbelt tests pass vacuously under a tmp root).
```

## Why

A refute pass on the kernel-target plan re-grounded its defect table against main. Every row is
still open, and two are worse than recorded. The fixes do not need the kernel rewrite. Each one
is a few lines at the site that owns the defect.

| # | defect | site |
|---|---|---|
| 1 | A `kernel://` read of a busy agent waits for the owner's whole cell (up to the 600 s ceiling), then fails with an empty message. | kernel/client.rs:846; runtime/kernel.rs:715-734 |
| 2 | Four writes are not atomic: the dump cell, the `rlm.put` sidecar, the snapshot manifest and the host reply spill. Two processes also share one `.tmp` name. | runtime/kernel_variables.rs:72-74; rlm/__init__.py:1014; kernel/snapshot.rs:119, 143-147; runtime/mail.rs:647-649 |
| 3 | `context_keys` values are cut at exactly 4,096 chars in Python, so the host's truncation marker never fires. The child gets invalid JSON with no marker. | rlm/__init__.py:449, 473; runtime/mailbox.rs:47-70 |
| 4 | A Seatbelted child kernel cannot write `family/`, so a child's `rlm.put` and a `kernel://<child>/var` object dump fail with EPERM on macOS. | runtime/kernel.rs:295-305; tools/sandbox.rs:29-48; wiring.rs:105-115 |
| 5 | Two processes continuing one session both write its snapshot, with no lock. | runtime/kernel.rs:354-440; kernel/snapshot.rs:119 |
| 6 | The kernel's `bash()` jobs run on the host unsandboxed, so a sandboxed kernel escapes its profile. | runtime/kernel.rs:37-55 (`spawn_job(…, None)`); wiring.rs:581 |
| 7 | Every cell, internal ones included, lands in the user's `~/.ipython` history DB. ipykernel is unpinned, although rlm patches its internals. | kernel/client.rs:242, 927; kernel/bootstrap.rs:13; rlm/__init__.py:351-362 |
| 8 | A variable over the 16 MiB snapshot cap is dill-dumped up to the cap on every checkpoint and never saved. | kernel/snapshot.rs:96-110 |
| 9 | Image attachments never reach the model, although the hint strings say `attach_image` puts them "in front of the model". | tools/ipython.rs:133-151; tools/document.rs:764; tools/hashline/documents.rs:86 |
| 10 | The family blackboard is keyed `rlm-<pid>`. It is lost on `--continue` and shared by every session in one `yi acp` / `yi serve` worker. | cli/main.rs:538; wiring.rs:105-115 |

## Rules every PR follows

- Open the forge issue first (D106). The body carries `Closes #N`.
- Bump the ARCHITECTURE version, add the CHANGELOG row, and update the feature-ledger row in the
  same diff. A D-row goes in only where the PR changes a settled decision (4, 5, 6 and 10). Its
  ADR goes under docs/solutions/adr/.
- Each regression test is seen red against unfixed main before the fix, per the testing doctrine.
- Real-kernel tests run under a temp HOME. A macOS-only test and its helpers share one
  `#[cfg(target_os = "macos")]`, and the Linux cross-lint runs before push.
- `just check` must be green by exit code. Kernel behavior changes also run the real binary
  offline (`./target/debug/yi ask --model faux/faux-1 …`) and `just journeys`.
- Size-ratchet baselines go in their own commit.

## PRs

### 1. Reads never wait behind a running cell (#507)

**Change.**
- In `KernelManager::execute`, race the execution-queue lock against `options.abort.fired()`.
- Return `aborted_result()` without sending anything if the abort wins. Dropping a lock future
  is cancel-safe; nothing has reached the kernel yet.
- In `KernelService::variable_cell`, start the 5 s timer before `manager_if_running()`. That
  mutex is held for a whole boot, so race it too.
- Map an aborted read to a new `VariableReadError::Busy`:
  "`<agent>` is running a cell; nothing was read (waited 5 s)". This replaces
  `Cell { detail: "" }`.

**Test** (runtime/tests/kernel_across_sessions.rs).
- The parent runs `time.sleep(30)`, and its child reads `kernel://main/x`.
- Expect `Busy` within 7 s.
- On today's code it returns after 30 s with an empty detail.

**Size and docs.** About 30 lines. Changelog row only.

### 2. Every shared file is written by rename (#508)

**Change.**
- The snapshot data tmp becomes `…tmp-<pid>`. The manifest is written to its own tmp, then
  `os.replace`d.
- The dump cell writes a tmp, then calls `os.replace`.
- The `rlm.put` sidecar is written to a tmp and `os.replace`d, still after the object, as today.
- The host spill in mail.rs writes each file to a tmp and renames it. Check the production
  duplication gate before adding a helper. If a second tmp+rename in yi-runtime trips it, lift
  one helper into the crate both callers reach (yi-ai's `write_atomic` is private today).

**Test.**
- Python unittest (`python/yi_runtime/tests/test_rlm.py`, fake host): make the sidecar write
  fail midway. The previous sidecar still parses. Today it is truncated.
- Runtime test: the same check for the manifest, driven through a failing manifest path.

**Size and docs.** About 40 lines. Changelog row only.

### 3. A cut `context_keys` value says it was cut (#509)

**Change.**
- Python cuts each value at `CONTEXT_VALUE_CAP + 1`, not `CONTEXT_VALUE_CAP`. That lets the
  host clamp in `mailbox::context_block`, the one place that writes the marker, fire.
- Comment the constant as the transport bound, not the policy.

**Test** (Python unittest, fake host).
- `rlm.run("…", context_keys=["xs"])` with `xs = list(range(3000))`.
- Expect the rendered brief to hold `[truncated to 4096 chars]`.
- Today the marker is absent.

**Size and docs.** About 3 lines. Changelog row only.

### 4. A sandboxed child can write its family board (D-row) (#510)

**Change.**
- `KernelService::kernel_wrap` pushes `options.family_dir` onto the profile's writable roots,
  beside `~/.yi/harness` and `~/.yi/mcp`.
- The root already reaches the board through the sessions dir; a child's `sub-*` root does not.

**Test** (runtime/tests/kernel_sandbox.rs, macOS).
- A child-shaped service has `session_dir = <scratch>/rlm-1/sub-a` and
  `family_dir = <scratch>/rlm-1/family`.
- `rlm.put("k", 1)` succeeds. Today it raises PermissionError.
- A write to `<scratch>/rlm-1/other` is still refused.

**Decision (D-row).** This restores the upward channel D164 designed: a child puts and the parent
`rlm.get`s, which unpickles bytes the child wrote. For a child that shares the parent's cwd,
that adds nothing: the child can already plant importable code there. For worktree and walled
(D216) children it is a new upward path. Default: grant it to every child, as D164 intended.
Alternative: grant it only to children whose cwd is the parent's.

**Size and docs.** About 5 lines, plus the D-row and ADR. It amends D87 (the kernel write set).

### 5. One writer per session snapshot (D-row) (#511)

**Change.**
- `KernelService::ensure_inner` opens `<id>.kernel-state.lock` beside the snapshot and calls
  `File::try_lock()` (std, flock on unix; toolchain 1.94). The manager holds the handle for its
  life.
- On contention, the kernel still restores, but its snapshot config gets no writes. The restore
  notice says another yi process owns this session's saved state.
- The OS releases the lock on exit, so there is no pid, no staleness rule and no zombie or
  pid-reuse trap.

**Test** (runtime/tests/snapshot_e2e.rs).
- Two services share one session dir and key. A runs `x = 1` and flushes. B runs `y = 2` and
  flushes.
- The snapshot names `x` only, and B's notice says it is not saving.
- Today B overwrites A. flock conflicts between two opens in one process, so this runs
  in-process.

**Decision (D-row).** It amends K10: a second continuation runs unsaved and says so.

**Size and docs.** About 35 lines, plus the D-row and ADR.

### 6. The kernel's `bash()` runs inside the kernel's sandbox (D-row) (#512)

**Change.**
- `HostRegistry::register_exec(cwd)` gains the kernel's `Option<Sandbox>`: the same profile
  `kernel_wrap` builds, with harness, mcp and the family root.
- wiring.rs:581 passes it through, and `exec.spawn` hands it to `spawn_job`.
- Off macOS, `Sandbox::available()` is false, so nothing changes there.

**Test** (runtime/tests/kernel_sandbox.rs, macOS).
- `await bash("touch <outside>")` leaves no file.
- Today the file appears.

**Decision (default taken 2026-09-24: contain it).**
- Today this is an escape hatch nobody chose. No D-row records it.
- Containing it removes network and out-of-root writes from kernel `bash()` on macOS.
- The bash tool's "ask on refusal" path does not exist here, so the job fails with
  `denial_hint`'s text.
- Default: contain it. Alternative: send `exec.spawn` through the permission broker the way the
  bash tool goes (larger).

**Size and docs.** About 25 lines, plus the D-row and ADR.

### 7. IPython keeps its history to itself; ipykernel is pinned (#513)

**Change.**
- Launch with `--HistoryManager.enabled=False` (client.rs:242). No cell reaches the user's
  `~/.ipython/profile_default/history.sqlite`, and kernels stop sharing one sqlite lock.
- If the test shows `~/.ipython` is still created, also set `IPYTHONDIR` under the kernel dir.
- Pin `IPYKERNEL_REQUIREMENT` to the major resolved today (read it from a fresh venv's
  `.bootstrap-version`).
- Extend `RUNTIME_READY_CHECK` to assert the ipykernel internals `_install_control_comm_handlers`
  touches. A breaking release then fails the boot loudly instead of hanging every host request.
- The ready-check string moves with crates/kernel/tests/ready_check.rs. The venv name hashes the
  check, so every machine rebuilds its venv once.

**Test.** Real kernel under a temp HOME: after one cell, `$HOME/.ipython/.../history.sqlite`
does not exist. Today it does, off macOS (the Seatbelt profile already refuses the write there).

**Size and docs.** About 10 lines. Changelog row only.

### 8. An over-cap variable is dumped once, not every checkpoint (#514)

**Change.**
- The generated snapshot code keeps `{name: id(value)}` of over-cap variables on a module that
  outlives the cell.
- It skips the dump while the id is unchanged, and still reports the variable as skipped.
- Mark it `ponytail:`: an object shrunk in place below the cap stays skipped until rebound.

**Test** (snapshot_e2e.rs).
- A namespace object whose `__reduce__` counts calls and yields 17 MiB.
- Two checkpoints give one call. Today they give two.

**Size and docs.** About 10 lines. Changelog row only.

### 9. Images a cell attaches reach the model (#515)

**Change.**
- `ipython.rs` appends a `Content::Image { data, mime_type }` block for each `image/*`
  attachment, under the existing K6 attachment cap.
- The providers already carry images in tool results: anthropic.rs:86-135 and openai.rs:237-260
  (a follow-up image message when the model takes images).
- This makes the two hint strings true, so they stay as they are.

**Test** (tools unit test).
- An `ExecuteResult` with one PNG attachment gives a tool output holding the image block.
- Today it holds text only.

**Size and docs.** About 15 lines. Changelog row, plus a feature-ledger row for `attach_image`.

### 10. The family board belongs to the root session (D-row) (#516)

**Change.**
- `family_dir` becomes `<sessions_dir>/family/<root-session-id>/`, carried down the wiring the
  way `depth` is. It is no longer derived from `rlm-<pid>`.
- mail.rs's spill takes the wiring's value.
- PR 4's writable root follows automatically, because it reads `options.family_dir`.
- Old `rlm-<pid>/family` dirs are left in place and not migrated.

**Test** (runtime/tests/family.rs).
- Two root sessions in one process: `rlm.put("k")` in A is invisible to B. Today B sees it.
- A continued session in a fresh wiring reads its own `k`. Today it is gone.

**Decision (D-row).** It amends D164's keying.

**Size and docs.** About 30 lines, plus the D-row and ADR. It lands after PR 4.

## Order

- **Independent, open in any order:** 1, 2, 3, 5, 7 and 9.
- **Serial where they share a site:**
  - 2 before 8 (both edit the generated snapshot code);
  - 4 before 10 (both touch the family root).
- **Waits on its decision:** 6.

Growth is about 200 lines of source across all ten, plus tests. Each PR prices its own growth
memo.

## Not in this plan

- **S1, S2 and S3** of `2026-09-11-kernel-target.md`: the kernel swap, per-cell history, rewind
  alignment and object handoff. All are deferred.
- **ACP notebook cells** are never persisted (acp/lib.rs:379-421). That is a feature, and needs
  its own issue.
- **harness.py removal** rides S1.
- **Write-only code:** `cellSourceCode` and `last_cell_code` (pump.rs:402-418, client.rs:964,
  1113), and the orphan journal with `YI_ORPHAN_JOURNAL` (journal.rs). Deleting the first pair is
  a free cleanup. The journal is a K8 feature, so cutting it waits for the owner's word (the
  feature-cut rule).
- **Noticed, not kernel:** `binary_size_budget.json` is 7,340,032 with no size-ledger row, while
  §13.6 still says 6 MiB. `.ruler/020` says "Fifteen crates"; there are 16.
