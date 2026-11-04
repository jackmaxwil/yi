---
version: 0.906.0
issue: Closes #1550
raise: tests +226
---
The checker recipe was a screen (`evals/checker_screen.py`) nobody could call from a session; it is now a skill, `skills/yi/checker/SKILL.md`, beside `unattended`. It says to write the requirement list from the task text, spawn one `role="worker"` child with the task text in its brief and `deny_url=["history://"]` so the child reads no transcript of the root's, of the root's parent or of a sibling, `deny_read` and `deny_write` on the workspace so it neither opens the attempt nor edits the graded tree, and have it write a `checks.py` of `PASS <id>` / `FAIL <id>` lines into a `checks_dir` the snippet binds outside the workspace. The root starts each requirement row with the check that proves it — a plan row's `check=`, its contract's `decider: {cmd}`, or the checks program itself where there is no plan — and the root runs the checks on the finished work itself: a reader has only `read` and `grep`, so it is handed the task text and the checks' output, reports the requirements they miss, and the root appends those rows; no row is relaxed and the work is not edited. Nothing is hidden from the implementer; a failing check reaches it as the check and its output.

The description opens with its condition, an interactive root or an allow rule, because a headless root refuses the child's `checks_dir` write and the catalog clips a line at 60 characters.

The skill is proven where a model would run it. Three tests install the shipped file under a scratch home: the catalog line carries the condition and `yi://skills/checker` reads back; the skill's own python fence runs in the kernel with a task holding a triple quote and the brief carries it unchanged with nothing executed; and the `rlm.run` payload walls history, refuses a read and a write of a workspace file, and leaves the `checks_dir` outside the workspace writable.
