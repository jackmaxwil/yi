---
issue: Closes #974
decision: a structural change adds one file under `docs/changes/` and never writes the `version:` line, a changelog row, a decision row or an ADR, which are written on main in merge order; the src growth, crate, test and comment ceilings are measured at the fork point and raised by a `raise:` line in that file | the owner: "We need to change how version and changelog are handled."; on 2026-09-30 all 9 open PRs that conflicted with main collided on these lines and baselines, which the forge cannot merge | restore the four baselines, `--update` for them and the bump rule in `.ruler/090`
---
A change is recorded in its own file under `docs/changes/` instead of a shared line, so two
pull requests no longer conflict on the version line, the changelog top, the decision table or
the size baselines (Closes #974). `check_changes.py` reads the file's header (`issue:`,
`growth:`, `raise:`, `decision:`) and refuses a branch that edits the version, a changelog row,
a decision row, an ADR or a change file main holds. `check_growth`, `check_crate_size`,
`check_test_size` and `check_comments` measure at `git merge-base origin/main HEAD` and accept
growth a change file raises, so `src_loc.json`, `crate_size_budget.json`,
`test_size_budget.json` and `comment_budget.json` are deleted with their `Ratchet:` commits.
Until the recorder (#975) lands, the version line and the changelog stay where they are and
change files wait for it.
