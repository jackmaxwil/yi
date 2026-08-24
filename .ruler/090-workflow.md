# Workflow

- A structural change bumps docs/ARCHITECTURE.md version and adds a changelog row in the same
  commit.
- Revising a settled decision requires a new D-row (decision, why, reversible-via) before code.
- Feature cuts are discussed before being written into the docs.
- One-in-one-out: adding a top-level feature deletes or demotes one and edits YI_DESIGN.md §1.1
  in the same commit.
- Instruction source of truth is .ruler/; generated AGENTS.md, CLAUDE.md, and propagated skill
  directories are untracked — edit .ruler and run `npx @intellectronica/ruler apply`, never the
  generated files.
- Commits: imperative subject; body only when the why is not obvious from the diff.
- A user-visible behavior change updates the ARCHITECTURE feature ledger and,
  when structural, the changelog — in the same change as the code.
