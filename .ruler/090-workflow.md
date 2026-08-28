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
- Commits: imperative subject; body only when the why is not obvious from the diff.
- Never include a "Co-Authored-By: Claude" trailer (or any assistant co-author trailer) in a
  commit message.
- Commit messages containing backticks or `$(` go through `git commit -F -` with a quoted
  heredoc, never `-m` — zsh command-substitutes inside double quotes and mangles the message.
- A user-visible behavior change updates the ARCHITECTURE feature ledger and,
  when structural, the changelog — in the same change as the code.
- A landed decision gets its ADR under docs/solutions/adr/ (one file per decision-log row,
  regenerated from the row rather than hand-drifted) and a line in the docs/solutions index.
- Never `git add -A` in a shared tree: it swallows the other session's uncommitted files. Stage
  the paths the change touched, by name.
