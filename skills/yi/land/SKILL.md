---
name: land
description: >
  Land a lane on this repository's forge: commit by name, the ratchet in
  its own commit, the changelog row, the ADR, push, open, wait, merge. Use
  when the user asks to commit, push, open a pull request, land, or merge.
  Do NOT commit, push or open anything the user did not ask for.
trigger: git commit, git push, tea pr, just land, open a pull request
scope: tool:bash
---

# land

`land` → stage by name → ratchets alone → docs in the same change → `just land` by parts

## Never without the user

Never commit, push, amend, force, rebase or skip hooks unless the user
asked for that action in this conversation. A lane is a branch off the
trunk; the trunk is the user's.

## Stage by name

`git add <path> <path>`; never `git add -A` or `git add .` in a tree
another session may share (a moved skill directory rode along this way
once). Read `git commit`'s own file list afterwards.

## Commit

- Subject ≤ 72 characters, imperative; body only when the why is not in
  the diff.
- No assistant co-author trailer; the `commit_style` gate refuses it.
- A message with backticks or `$(` goes through `git commit -F -` with a
  quoted heredoc, never `-m`:

```
git commit -q -F - <<'EOF'
Subject line

Body.
EOF
```

- The pre-commit hook runs fmt, clippy and the fast guardrails; a red
  gate after your change is your change. Fix the code, not the baseline.

## Ratchets

A baseline never rides the code commit. Order: land the code red on the
baselines, then `python3 scripts/guardrails/<gate>.py --update` and commit
the baseline files alone, subject `Ratchet: …`. The hook checks
`HEAD..origin/main` for style: a subject over 72 characters anywhere in
the branch blocks every later commit; recreate the branch tip with
`git update-ref` and `git restore --staged`, never rebase.

## Docs in the same change

A structural change bumps `version:` in `docs/ARCHITECTURE.md`, adds a
row to `docs/CHANGELOG.md` with a `growth +N:` memo when the version's src
growth passes the free band (`python3 scripts/guardrails/check_growth.py`
prints N), adds its D-row above the last one, and renders the ADR with
`python3 scripts/adr.py <N>`. A user-visible behaviour change updates the
feature ledger. Read the header and the last D-row immediately before
writing them; another session may have claimed the number.

## Push and open

`just land` is the whole dance: merge `origin/main`, ratchet, reprice the
growth memo, render missing ADRs, push, open, wait, merge. When the ADR
backlog breaks it, land by parts: `just commit`, `just adr <N>`, `just pr
open`, `just pr merge`. The push runs a pre-push lane that takes minutes;
export `UV_CACHE_DIR` before it or the cli_surfaces tests time out.

Behind base after a predecessor lands: `POST pulls/N/update?style=merge`
on the forge, then wait for CI again. CI logs: `just ci-log` in the ops
repository.

## Report

The commit hashes, the PR URL, and the gate's exit line; never "pushed"
without the push's own output.
