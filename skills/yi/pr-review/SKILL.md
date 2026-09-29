---
name: pr-review
description: >
  Review the open draft PRs on this repository's forge in rounds, and fix what a round
  confirms when the owner has turned the fixer on. Use when a todo from the forge's
  `/pulls` channel wakes you ("N message(s) from forgejo://…/pulls"), or when asked to
  review, re-review or sweep PRs. Do NOT merge anything, and do NOT edit the probe files
  under this skill to make a round pass.
---

# Reviewing drafts in rounds

A draft PR (`WIP: ` title) leaves draft after two review rounds on its head, the last not
blocked. Each round is one comment whose first line is `<!-- yi-round N -->`; the repository's
`scripts/pr_review.py` writes it (D308). The probes it runs are the files in `probes/` beside this
skill: one question each, a brief, a severity scale, and `when` rules the code evaluates.

## When the forge channel wakes you

Run one sweep from the repository root and report what it printed:

```bash
just pr sweep
```

The sweep reviews every open draft whose head no round has read enough, and skips the rest, so
a repeated or late message does nothing. It never merges. It runs the fixer only when
`YI_REVIEW_FIX=1` is set, which is the owner's switch, not yours.

## By hand

- `just pr review N` posts one round; `--dry-run` prints it instead; `--again` reads a head the
  rule says is read enough.
- `just pr fix N` hands the last round's high and medium findings to a fresh fixer on the PR
  branch; it refuses a branch outside this repository, a PR that is not a draft, and a head that
  is already its answer to that round.
- `just pr ready N` lists what a draft still owes.

## Subscribing a reviewer session

In a session that should keep reviewing, from its kernel:

```python
await rlm.subscribe(
    "forgejo://git.example.invalid/apex/yi/pulls?every=60s",
    create={"label": "review the drafts whose head moved", "note": "Run `just pr sweep` (skill pr-review)."},
    filter="draft=true",
)
```

The review model must come from a family other than the authors' (`YI_REVIEW_MODEL`; the
default avoid list is `anthropic`, since this repository's PRs are written on Anthropic models).
