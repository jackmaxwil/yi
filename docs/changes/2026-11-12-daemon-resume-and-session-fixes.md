---
issue: Closes #2166
decision: Double Ctrl+C in the console detaches and leaves `yi serve` running (amends D356's parenthetical "ctrl+c twice in the console" and the console quit note of #1032/#1350); stopping the daemon stays on the command palette's `stop the daemon and quit` (`_yi/shutdown`) | a double Ctrl+C meant to leave a pane killed the shared daemon and every worker and background session on the machine | restore `self.stop_daemon_and_quit(outbound)` in `crates/console/src/app.rs` on the second Ctrl+C press
ledger~: TUI | the daemon that second press stops ends the console when it hangs up, with no disconnect banner | the second press quits the console while leaving the daemon running, and stopping the daemon from the palette ends the console when it hangs up, with no disconnect banner
ledger+: daemon (ACP-router supervisor + worker/root) | a killed daemon is restarted by a connected console's reader loop and any request naming a stored session spawns its root's worker and loads the session from disk without needing a fresh `session/resume` first (`serve_daemon::prompting_a_session_after_the_daemon_was_killed_resumes_and_answers`, T2)
growth: +324 daemon resume, reactor handle for plain-thread wakes with abandon_admit on detach, closed-set so ensure_loaded does not revive a closed session, plan start on contracted running, shadow-snapshot .git, and forge/wt_home lane fixes; weighed deleting the settings seam move back into daemon.rs (keeps file_size/fn_size under cap) and collapsing ensure_loaded into open_later call sites (leaves every prompt/control path without a single load point)
cut: none crates/console/src/app.rs
cut: none crates/console/tests/drive.rs
cut: none crates/runtime/src/session/run.rs
cut: none scripts/forge_pr.py
raise: crate acp +132, crate cli +13, crate console +110, crate runtime +61, crate tools +8, tests +491, comments +10
---

Double Ctrl+C in the console now quits the console and leaves the daemon and its workers running, matching `⌥q`; stopping the daemon stays in the command palette (`stop the daemon and quit`).

Killing `yi serve` mid-session no longer strands open panes on `Prompt: unknown sessionId`. The console's socket reader restarts the daemon when its connection drops (unless the console itself asked the daemon to stop), the supervisor resolves unledgered sessions from disk and spawns a dead or restarted root's worker on any session-addressed request, and a worker loads a stored session on demand when a prompt or control call arrives ahead of `session/resume` — except after an explicit `session/close`/`session/delete` on that worker, which stays closed until `session/resume`.

Four more bugs surfaced by the latest Yi sessions and `~/.yi/daemon.worker.log` are fixed alongside it:
- `AgentSession` records its Tokio runtime handle and spawns turns through it instead of bare `tokio::spawn`, fixing the worker panic (`there is no reactor running`) when a plain OS thread (`LaneHandle::land`, an adapter supervisor or a background slash thread) wakes or queues an idle session.
- `plan set` moving a contracted todo directly to `running` now runs `Op::Start` so `contract_hash` is frozen and a later `done` is no longer refused with `the contract was never frozen; start records it`.
- `Checkpoints::materialize` initializes a disposable `.git` inside a materialized snapshot of a git workspace so git-aware deciders (`scripts/wt_home.sh`) no longer exit 128 on `git rev-parse --show-toplevel`.
- `scripts/wt_home.sh` links `~/.config/fgj` into the scratch `HOME`, and `scripts/forge_pr.py` ignores untracked `.yi/plans/` in `just push`, wires the zig cross-compiler environment automatically for `linux-clippy` on macOS, and accepts inline text as well as a file path on `--body`.
