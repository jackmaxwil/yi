# Workflow

- A structural change adds one change file, `docs/changes/<yyyy-mm-dd>-<slug>.md`: a `---` header
  (`issue:`, `growth:`, `raise:`, `decision: <decision> | <why> | <reversible via>`, each optional)
  and the changelog prose. It never writes the `version:` line, a CHANGELOG.md row, a decision row
  or an ADR: those are views the recorder writes on main after the merge, numbering versions and
  D-rows in merge order, and check_changes.py refuses a branch that edits them. Every PR claimed
  the next number by hand and every landing renumbered, until 9 of 9 conflicted PRs collided on it.
- Revising a settled decision requires a new decision (a `decision:` line in the change file) before code.
- What a change removes or holds on purpose is recorded where every later branch is checked against it:
  `cut: <path> | <the deleted line, verbatim> | <why>` and `keep: <path> | <line> | <why>` in the change
  file. check_intent.py refuses any branch, by any author, that puts a cut line back or deletes a kept one,
  unless its change file says `revives:`/`drops: <change file> <path>` beside a `decision:`; `just pr open`
  asks for a `cut:` line, or `cut: none <path>`, for each file that loses five or more lines on net.
  Incident: a fixer and two sessions undid three of the owner's deliberate cuts in one week.
- One open pull request owns a file. `just pr open` refuses a branch that changes a file another open PR
  changes, naming both numbers and the files, unless one is stacked on the other or the change file says
  `shares: #N <why>`; a new lane or land-fix runs `scripts/owners.py check` the same way. Files the
  recorder or a ratchet writes for everyone (change files, ledger lines, baselines, Cargo.lock) own nothing.
- Feature cuts are discussed before being written into the docs.
- One-in-one-out: adding a top-level feature deletes or demotes one and edits YI_DESIGN.md §1.2
  in the same commit.
- Instruction source of truth is .ruler/; edit .ruler and run `npx @intellectronica/ruler apply`,
  never the generated files. AGENTS.md is tracked so a lane checkout carries the rules (commit it
  with the rule; `check_agents_md.py` fails when it lags); CLAUDE.md and skill copies are not.
- A commit subject and a PR title are the same thing: one plain imperative sentence, at most 72
  characters, first word capitalized, no terminal period, and self-evident to a cold reader —
  "Refuse the next done-claim on a rung-refused task", never "Close N14" or "Address feedback".
  Ids belong in the body; a title that is an id names nothing. The one prefix is `Ratchet: `,
  always carrying its measured `X -> Y`, beside the `Merge`/`Revert` subjects git writes itself;
  every other `word:` prefix fails, conventional-commit dialect included. Body only when the why
  is not obvious from the diff. check_commit_style.py gates `HEAD --not origin/main`, and the
  same script reads `PR_TITLE` so CI judges the title by the identical rule.
- No trailers, with four carve-outs: the flywheel plan's §8 `Opt-*` set on optimizer commits, one
  `Plan: plan://<plan>/<todo>` on a commit that lands work under a plan, the trailers git writes
  itself, and `GitHub-Commit: <40-hex sha>`, written by yi-bot's history-preserving GitHub import
  on an imported commit to record the GitHub-side commit it came from. The current squash intake in
  `scripts/intake/bridge.py` spells that source head `GitHub-Head:`. Assistant co-author trailers
  stay banned outright. The `Plan:` value is checked as a URL, because a trailer nothing can resolve
  indexes nothing: it is what makes `git blame` → commit → todo → goal → the user's own words resolve
  with no inference, and the index only holds what was trailered when it landed, so it is written
  from the first such commit rather than added later.
- A PR body is the cold-reader narrative in .github/PULL_REQUEST_TEMPLATE.md — why needed,
  summary, user outcomes, seen red, UI changes, files-edited map, schema changes, deleted /
  alternatives, risk and rollback, performance, architecture notes — every section written and
  no template comment left, with zero checkboxes: gate proof lives in the CI status checks
  alone, where it cannot be ticked by hand. A PR opens as a `WIP: ` draft; `just pr ready N`
  takes the marker off.
- Commit messages containing backticks or `$(` go through `git commit -F -` with a quoted
  heredoc, never `-m` — zsh command-substitutes inside double quotes and mangles the message.
- A change that adds a feature-ledger row, or whose net src growth exceeds the free band, carries
  `Closes #N` or `Refs #N` in its PR body and cites the same `#N` in its change file — the
  register is on the forge (095), so the change and the issue have to name each other or neither
  can be found from the other. Ratchets, doc fixes and in-band repairs are exempt by
  construction: they add no ledger row and move no bytes past the band.
- A user-visible behavior change writes `ledger+`, `ledger~`, `ledger=` or `ledger-` lines in its
  change file, in the same change as the code. The recorder applies them to the ARCHITECTURE
  feature ledger after the merge; a branch that edits a row directly is refused and told the lines
  to write, whenever that edit was committed.
- A landed decision gets its ADR under docs/solutions/adr/ (one file per decision-log row,
  rendered from the row by the recorder rather than hand-drifted) and a line in the docs/solutions index.
- Never `git add -A` in a shared tree: it swallows the other session's uncommitted files. Stage
  the paths the change touched, by name — and check `git commit`'s own file list afterwards: a
  deletion another session staged rides along silently otherwise (a moved skill did exactly this).
- A PR that adds or changes a test fills `## Seen red`: per test, one line — the failure it
  produced against the unfixed code and the fixture's provenance. It is not gate output; it is
  the author's claim, and the reviewer's cold-context read is what judges it.
