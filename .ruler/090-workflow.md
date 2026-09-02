# Workflow

- A structural change bumps docs/ARCHITECTURE.md version and adds a changelog row in the same
  commit. Read the version header and the last D-row immediately before writing them: another
  session sharing this tree may have claimed both since the last read, and a collision costs a
  reset + renumber (0.35.0/D55 and 0.38.0/D57 were both taken mid-change this way).
- Revising a settled decision requires a new D-row (decision, why, reversible-via) before code.
- Feature cuts are discussed before being written into the docs.
- One-in-one-out: adding a top-level feature deletes or demotes one and edits YI_DESIGN.md §1.1
  in the same commit.
- Instruction source of truth is .ruler/; generated AGENTS.md, CLAUDE.md, and propagated skill
  directories are untracked — edit .ruler and run `npx @intellectronica/ruler apply`, never the
  generated files.
- A commit subject and a PR title are the same thing: one plain imperative sentence, at most 72
  characters, first word capitalized, no terminal period, and self-evident to a cold reader —
  "Refuse the next done-claim on a rung-refused task", never "Close N14" or "Address feedback".
  Ids belong in the body; a title that is an id names nothing. The one prefix is `Ratchet: `,
  always carrying its measured `X -> Y`, beside the `Merge`/`Revert` subjects git writes itself;
  every other `word:` prefix fails, conventional-commit dialect included. Body only when the why
  is not obvious from the diff. check_commit_style.py gates `HEAD --not origin/main`, and the
  same script reads `PR_TITLE` so CI judges the title by the identical rule.
- No trailers, with three carve-outs: the flywheel plan's §8 `Opt-*` set on optimizer commits, one
  `Plan: plan://<plan>/<todo>` on a commit that lands work under a plan, and the trailers git writes
  itself. Assistant co-author trailers stay banned outright. The `Plan:` value is checked as a URL,
  because a trailer nothing can resolve indexes nothing: it is what makes `git blame` → commit →
  todo → goal → the user's own words resolve with no inference, and the index only holds what was
  trailered when it landed, so it is written from the first such commit rather than added later.
- A PR body is the cold-reader narrative in .github/PULL_REQUEST_TEMPLATE.md — summary, user
  outcomes, UI changes, files-edited map, schema changes, LOC and justification, architecture
  notes, screenshots — with zero checkboxes: gate proof lives in the CI status checks alone,
  where it cannot be ticked by hand.
- Commit messages containing backticks or `$(` go through `git commit -F -` with a quoted
  heredoc, never `-m` — zsh command-substitutes inside double quotes and mangles the message.
- A change that adds a feature-ledger row, or whose net src growth exceeds the free band, carries
  `Closes #N` or `Refs #N` in its PR body and cites the same `#N` in its changelog row — the
  register is on the forge (095), so the row and the issue have to name each other or neither
  can be found from the other. Ratchets, doc fixes and in-band repairs are exempt by
  construction: they add no ledger row and move no bytes past the band.
- A user-visible behavior change updates the ARCHITECTURE feature ledger and,
  when structural, the changelog — in the same change as the code.
- A landed decision gets its ADR under docs/solutions/adr/ (one file per decision-log row,
  regenerated from the row rather than hand-drifted) and a line in the docs/solutions index.
- Never `git add -A` in a shared tree: it swallows the other session's uncommitted files. Stage
  the paths the change touched, by name — and check `git commit`'s own file list afterwards: a
  deletion another session staged rides along silently otherwise (a moved skill did exactly this).
