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
`scripts/pr_review.py` writes it (D319). The probes it runs are the files in `probes/` beside this
skill: one question each, a brief, a severity scale, and `when` rules the code evaluates.

## When the forge channel wakes you

Run one sweep from the repository root and report what it printed:

```bash
just pr sweep
```

The sweep reviews every open draft whose head no round has read enough, and skips the rest, so
a repeated or late message does nothing. It never merges. A blocked round is the autofixer's
(`just pr autofix N`), which the `autofix` workflow runs every 15 minutes.

## By hand

- `just pr review N` posts one round; `--dry-run` prints it instead; `--again` reads a head the
  rule says is read enough.
- `just pr autofix N` answers the last round's findings, or a conflict, on the PR branch now;
  `just pr spend` totals what the bots spent, from their comments' meta lines.
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
