# Yi: a PR lifecycle as a plan on the forge channel

```
status:  PROPOSAL, 2026-09-26. Re-read against origin/main @ e0da990e (#658, 0.375.0)
         on 2026-09-27. Stage issues: #667, #668, #669, #670, #679, #680.
depends: docs/plans/2026-09-24-seven-primitives.md, built first (owner decision below).
         Stages 0-3 here run as the prompt-triggered case and need none of it.
marks:   ✓ exists on main · ✚ new here · ◇ new in seven-primitives, not yet built
```

## 0. Summary

Every PR opens as a draft (`WIP:` title). A saved plan definition, `pr-lifecycle`, takes
it through intake, review round 1, a fixer, and review round 2. A PR becomes ready only
when round 2 is clean on the current head. Each round is a todo. Its verdict goes out as a
comment with a marker, pinned to a head sha. You, a Claude Code session or the bot can
write that comment. The bot never merges.

No new primitive is needed. Every part is a composition of things that exist or that the
seven-primitives plan already builds:

| piece | composed from | status |
| --- | --- | --- |
| the lifecycle | a plan definition `pr-lifecycle`, `Plan.create(request_id="pr/<n>")` | ✓ plan engine, ✚ definition |
| trigger | subscription on `channel://forge/apex/yi?event=opened` via the forge adapter | ◇ stage 6 adapters |
| same, before channels | `just pr review <n>` runs the same definition by prompt (§5.1 trigger axis) | ✚ verb |
| intake | `run=` todos: template check, duplicate check, existing gates. Zero tokens | ✓ parts, ✚ two checks |
| review round | `review_pod` readers, one brief per lens | ✓ recipe, ✚ briefs |
| refute | one refuter per finding; the host verifies a quote or a command (§7.3) | ✓ `verify_quotes`, ◇ command repro |
| necessity | the intent judge at the "done is claimed" boundary (§6.4-6.5) | ◇ stage 2 |
| fixer | the review_pod arbiter: a Writer in the PR worktree, `accept = just check` | ✓ |
| round comment | outward view of the round todo's verdict; `forgejo_pr_comment.upsert` today, adapter later | ✓ upsert, ✚ marker |
| self-authored round | a marker comment on the channel completes the round todo as an external result | ◇ channel input |
| stale rule | a todo `Blocked { on: Channel { event=synchronize } }` → delta round | ◇ |
| ready | adapter edits the title to strip `WIP:` | ◇ adapter outward |
| merge | `Blocked { on: User }`, and only the owner unblocks it | ✓ |
| lease | per-PR lease sized by intake class (§7.6) | ✓ tokens, ◇ dollars |

## 1. The problem

### 1.1 Nobody reviews, and merges are fast

Counted on the forge (apex/yi, `fgj api repos/apex/yi/pulls`, 2026-09-26):

- 298 PRs: 269 merged, 23 closed unmerged, 6 open.
- The last 80 merged PRs (#395-#597) got **1 formal review**. 14 carry any comment from a
  person; the other 160 comments come from `forgejo-actions` (size report and live lane).
- Median time from open to merge is **0.66 h** (p10 0.04 h, p90 19.9 h). 147 of 269 merged
  PRs landed in under an hour.
- Sizing, from the last 92 merged: median 235 changed lines and 12 files, p90 922.
  10 of 92 were 50 lines or fewer.

### 1.2 Green gates let bad work through

- **#334-#364** merged green, were bad, and main was reset to `ebbfba3` (#333) on
  2026-09-09. Every gate passed on every one of them.
- The plan-engine landing (2026-09-01): six review lenses produced 32 candidate findings. A
  refute pass that had to reproduce each one confirmed 20 and killed 12. The 20 sat under
  the builders' own passing tests.

### 1.3 Thrash and duplicates

The compaction cluster (#159, #170, #172, #174) and the sweep cluster (#405, #406, #409)
all closed unmerged. No check ever compares an open PR with another PR.

### 1.4 The template is present but unfilled

In the last 80 merged bodies, every heading is present (one body lacks "Schema changes", one lacks "LOC and justification"), yet **30 still contain `<!-- -->`
placeholders**. The size-report comment on #549 and #560 shows `<!-- why this area was
touched -->` left in "Files edited". No check reads the template's headings:
`check_pr_metadata.section_table_ok` (`scripts/guardrails/check_pr_metadata.py:158`) only
checks that one named section holds a table, and only `surface_problems` calls it.

### 1.5 Two collisions in today's code

- Forgejo 15 (15.0.7) marks a draft only by a `WIP:` title prefix, and
  `SCRATCH` (`scripts/guardrails/check_commit_style.py:38`) makes the `title` job fail on it.
- yi's own `/land` opens PRs with an empty body (`crates/runtime/src/lane/land.rs:122`,
  `:129`), so any PR it opens that owes `Closes #N` already fails the template.

## 2. Laws

1. **Channels first.** The owner: "docs/plans/2026-09-24-seven-primitives.md is an open
   plan that includes creating channels. assume that plan will be built out first". The bot
   is a subscription on the forge channel, not a CI job. Before channels exist, the same
   definition runs by prompt.
2. **The bot never merges.** #334-#364 were merges nobody authorized. Merge is a todo
   blocked on the owner.
3. **One ledger.** A round comment is a view of a round todo in the plan's journal. It is
   never a second store. A self-authored round is input on the channel, recorded in the
   same journal.
4. **A finding counts only with evidence the host can check**: a quote that
   `verify_quotes` finds, or a command that reproduces. Unverified findings are dropped
   before the fixer sees them.
5. **Never relax linters.** The fixer's wall denies hand edits to guardrail baselines,
   `scripts/guardrails/`, and `.forgejo/`. A red lint is fixed in the code. The one
   baseline writer it may run is `just ratchet`, in its own commit (§3.6).
6. **No assistant attribution.** Fixer commits pass `check_commit_style` like any other:
   imperative subject, no co-author trailer.
7. **Judges route** (the seven-primitives amendment). A confirmed high finding routes the PR
   back. That is the one place a model verdict controls flow, and it is bounded by the
   round cap.

## 3. Shapes

### 3.1 The round (a todo, rendered as a comment)

A round is a todo whose work is a review pod over one head sha. Its output renders as one
upserted comment whose first line is the marker:

```
<!-- yi-round pr=588 n=2 sha=4f1c2e9 base=3503ed74 verdict=clean high=0 medium=1 low=3 mode=shadow plan=plan://pr-588 -->
```

- `verdict` is `clean`, `blocked`, or `override`. `mode` is `shadow` or `blocking`.
- The upsert key is `(pr, n)`, and a new sha gets a new round. A round on a sha that is no
  longer the head is stale by construction.
- **Self-authored rounds.** A comment whose first line is a valid marker completes the
  round todo when it comes from an allowed account: the repo owner or the bot account.
  Markers anywhere else are ignored: in a PR body, in a diff, from another account. Anyone
  can write text that looks like a marker, so the author check is what makes it a round.
- **Override.** `/override <reason>` from the owner turns a blocked round into
  `verdict=override`. The reason is recorded in the journal and quoted in the comment.

### 3.2 Findings

One schema for every lens and every deterministic check:

```
Finding ✚ {
  lens:      "correctness" | "tests" | "scope" | "subtract" | "naming" | "perf" | "necessity"
             | "template" | "duplicate" | "gate"
  severity:  high | medium | low
  claim:     one sentence
  evidence:  [Quote { url, line, text }] | Repro { cmd, expect }
  fix:       optional, a proposed change the fixer may take
  refuted:   bool, set by the refute pass
}
```

### 3.3 Lenses

The owner's list, mapped to briefs. A brief is one entry in `BRIEFS`
(`python/yi_runtime/src/yi/recipes/review_pod.py:18`), which makes it data, not code.

| owner asked for | lens | decider | default severity | status |
| --- | --- | --- | --- | --- |
| bugs | correctness | quote or repro | high | ✓ brief |
| regressions | tests, plus the "Risk and rollback" claim | a repro that is red on head | high | ✓ brief, ✚ claim |
| overcomplexity, ways to reduce LOC | **subtract** (MERGE: one lens, because both answer "what can go") | quote plus a proposed deletion | medium | ✚ brief, from `skills/yi/simplify` |
| naming issues | naming | quote | low; medium on a public API or schema | ✚ brief |
| performance drains | perf, plus the "Performance" claim | a probe command with a number | high if measured, else medium | ✚ brief |
| is the PR necessary | necessity: the intent judge on "Why needed" and the linked issue | judge (◇), a brief until then | high | ◇ |
| (existing) | scope | quote | medium | ✓ brief |

The severity tiers (owner: "Severity tiers"):

- **high** blocks ready: a bug, a regression, an unnecessary PR, a duplicate.
- **medium** goes to the fixer. If the fixer declines with a reason, the finding does not
  block.
- **low** is listed in the comment and never blocks and never goes to the fixer.

### 3.4 Refute

§7.3 applied per finding. Each refuter is prompted to default to refuted, and the host keeps
a finding only if its quote verifies or its command reproduces. The refute budget follows
severity, because a high finding blocks: one refuter for low and medium, and three for high,
kept on two of three. That matches the workflow that produced the 20-of-32 result.

### 3.5 Intake (deterministic, zero tokens)

| check | how | on failure |
| --- | --- | --- |
| template present and filled | every required `##` section exists, is non-empty, and contains no `<!--` left from the template. New: no template-heading check exists | high, `lens=template` |
| duplicate | (a) another open PR cites the same `Closes #N`; (b) the set of definitions this PR touches (`grid diff`) overlaps another open or recently closed PR above a threshold; (c) windowed hashes of added lines (`check_duplication.py`'s normalized windows) match another PR's diff | high, `lens=duplicate`, names the other PR |
| existing gates | the required contexts on main (`gate (lint/guardrails/test)`, `size-report`, `title`) | the round waits; a red gate is a finding, `lens=gate` |

Unnecessary PRs and duplicates both block, and the owner clears them with `/override`
(owner: "Block + owner override"). Nothing closes automatically.

### 3.6 The fixer

The review_pod arbiter is already a Writer in its own worktree whose verdict is a
command. The fixer is that arbiter, seated fresh every time (owner: "Fresh fixer always").
It reads only the confirmed findings, the diff and the intent record, and never the authoring
session. It commits on the PR branch through the normal hooks and pushes (owner: "Auto-push
to PR branch"). Its accept is `just check`. Its wall denies hand edits to baselines,
`scripts/guardrails/` and `.forgejo/`. A fix that grows a ratcheted count lands a
`just ratchet` commit ahead of its code commit, because almost every fix adds a test and a
fixer that could not ratchet would hand nearly every fix back; the next delta round reviews
that commit like any other. Growth past the free band owes a priced changelog memo, so that
fix goes to the owner. It runs once per round, and the lifecycle caps at three rounds
before it hands the PR to the owner.

### 3.7 Stale and delta rounds

Any push after a round, whether from you, Claude Code or the fixer, makes that round stale.
The next round is a delta (owner: "Delta round"): intake reruns in full, and every lens
reviews `sha_last_clean..head` plus the files it touches. The first round of a PR is always
full.

### 3.8 Draft and ready

Every PR opens as `WIP: <subject>`. Forgejo disables the merge button on it natively.
The `title` rule accepts exactly `WIP: ` in front of a subject and judges the rest.
`WIP`, `wip` and `fixup!` stay refused everywhere else. When round 2 or later is clean
on the head, the lifecycle strips the prefix (owner: "WIP prefix + bot strips"). The title
edit has to re-run the `title` job, so that job listens for `edited` (open question 1).

### 3.9 The lease

The lease is sized by intake class (§7.6), with the class taken from the diff:

| class | from | review |
| --- | --- | --- |
| trivial | ≤50 changed lines (10 of the last 92 merged PRs) | intake plus the correctness lens; no fixer unless it finds something high |
| standard | ≤922 lines (p90) | intake, every lens, refuters, fixer |
| large | >922 lines | the same, plus three refuters on every medium finding too |

Dollar caps per class are set from the shadow phase's measured cost, not guessed here. The
journal records every run's spend, so the cap comes from a query.

## 4. Worked example

Illustrative: the title is open PR #588's, the findings are invented to show the flow.

Claude Code finishes "Keep streamed markdown where the reader saw it" and runs
`just pr open`. The PR opens as `WIP: Keep streamed markdown where the reader saw it`.

1. The forge adapter appends `opened pr=588` to `channel://forge/apex/yi`. The subscription
   starts `pr-lifecycle` with `request_id=pr/588`, so a reopen attaches to the same plan.
2. **Intake.** The template check finds "Performance" empty. It records a high template
   finding. The duplicate check finds no other PR citing the same issue, and a `grid diff`
   overlap with #575 of 1 definition out of 9, below the threshold. The gates are green.
3. **Round 1** on `4f1c2e9`. The correctness reader quotes a line where a resize drops the
   anchor. The subtract reader proposes deleting a helper that duplicates `scroll_to`. The
   naming reader flags `tmp_anchor2` (low). The necessity reader is satisfied by the
   "Why needed" section, which cites the issue and the owner's words.
4. **Refute.** Three refuters try to break the resize finding. Two reproduce it with a
   headless frame test, so it stays high. The subtract finding verifies as a quote, so it
   stays medium. The comment posts
   `<!-- yi-round pr=588 n=1 sha=4f1c2e9 verdict=blocked high=2 medium=1 low=1 ... -->`.
5. **Fixer.** A fresh Writer in the PR worktree gets two findings: the template gap and the
   anchor bug. The low naming finding is only listed. It adds the test, fixes the anchor,
   deletes the helper, and fills "Performance" with "no hot path touched". The new test
   grows the test-size count, so `just ratchet` lands first; then `just check` passes, and
   it pushes `9a0d3b1`.
6. The push lands on the channel, round 1's `synchronize` wait fires, and **round 2** runs
   as a delta over `4f1c2e9..9a0d3b1`. It comes back clean, and the comment posts with
   `verdict=clean`.
7. The adapter strips `WIP:`, and the `title` job re-runs on `edited`. The merge todo now
   waits on you. You merge, or you push one more commit and a delta round runs first.

## 5. The template, v2

Four sections are added (owner: "Why needed, Deleted / alternatives, Risk and rollback,
Performance"). Each gives one lens an author claim to test.

| section | the author states | tested by |
| --- | --- | --- |
| **Why needed** ✚ | `Closes/Refs #N`, the quoted owner words or issue line it answers, and what happens if it doesn't land | necessity, duplicate |
| Summary ✓ | what changed and why | all |
| User outcomes ✓ | unchanged | correctness |
| Seen red ✓ | unchanged | tests |
| **Deleted / alternatives** ✚ | what the change removes, and the simpler options weighed. Absorbs "LOC and justification" | subtract |
| **Risk and rollback** ✚ | what could regress, how you would notice, how to revert; durable-data and schema moves | tests (regressions) |
| **Performance** ✚ | the hot path touched, with a measured number, or "no hot path touched" | perf |
| UI changes ✓ | absorbs "Screenshots" | correctness |
| Files edited ✓ | unchanged, prefilled by `just pr-body` | scope |
| Schema changes ✓ | unchanged | tests |
| Architecture notes ✓ | unchanged | scope |

"Screenshots" merges into "UI changes", and "LOC and justification" merges into "Deleted /
alternatives". The net change is two more headings. The prefilled halves from
`scripts/pr_body.py:207-287` move with their sections.

## 6. Exists and new

| piece | where | status |
| --- | --- | --- |
| review pod, readers and arbiter | `python/yi_runtime/src/yi/recipes/review_pod.py:18-93` | ✓ |
| quote verification | `yi.roles.verify_quotes` | ✓ |
| jury tier, walled readers from another family | `crates/runtime/src/plan/judge.rs` | ✓ |
| comment upsert with marker | `scripts/forgejo_pr_comment.py:33` | ✓ |
| forge decision function | `scripts/forge_pr.py:129` `decide` | ✓ |
| PR lookup | `scripts/forge_pr.py:90` `pull_for_branch` | ✓ |
| body checks | `scripts/guardrails/check_pr_metadata.py:158`, `:190`, `:209` | ✓, extended |
| title rule | `scripts/guardrails/check_commit_style.py:38` `SCRATCH` | ✓, amended |
| windowed hash | `scripts/guardrails/check_duplication.py` | ✓, reused |
| definition diff | `grid diff` (read-only, `crates/tools/src/builtins.rs:184`) | ✓ |
| one-shot structured run | `yi ask --schema --json` (`emit_structured`, `crates/cli/src/main.rs:947`) | ✓ |
| simplify standard | `skills/yi/simplify/SKILL.md` | ✓ source of the subtract brief |
| lens briefs: subtract, naming, perf, necessity | `BRIEFS` entries | ✚ |
| round marker and parser | about 40 lines beside `forgejo_pr_comment.py` | ✚ |
| `pr-lifecycle` definition | one Python plan program | ✚ |
| template v2 and filled check | `.github/PULL_REQUEST_TEMPLATE.md`, `check_pr_metadata.py` | ✚ |
| duplicate check | one intake script | ✚ |
| forge adapter | seven-primitives stage 6 (`github` adapter, speaking Gitea/Forgejo) | ◇ |
| channel-blocked todos (`BlockedOn::Channel`) | seven-primitives stage 4 | ◇ |
| intent judge | seven-primitives stage 2 | ◇ |
| dollar leases | seven-primitives §3.6, not yet in a stage | ◇ |

## 7. What this deletes

- `crates/runtime/src/lane/land.rs` `poll_forge` and `poll_until_merged` (`:221`, `:543`):
  a 60 s poll for up to 2 h. They become a todo blocked on the forge channel.
- The poll loop in `scripts/forge_pr.py:469` (`just pr merge`, 20 s for up to 40 min) becomes
  the same blocked todo. `just pr merge` stays as the owner's unblock.
- `just pr rerun` (close and reopen to re-trigger), once the adapter can request a rerun.
- `/land`'s empty-body `pr create` (`land.rs:122`, `:129`) is replaced by `just pr open`'s
  body path.

## 8. Build order

Each stage has a demo and a gate, and each has an issue. Stages 0-3 need no new
infrastructure and go first at the worst pain, unreviewed merges. They run the definition
by prompt through a new verb, `just pr review <n>` (stage 1), the degenerate trigger.

| stage | scope | demo | gate |
| --- | --- | --- | --- |
| **0. Template and draft** (#667) | template v2; the filled check; `WIP: ` title rule; `/land` body fix | the 30 placeholder bodies go red under the check | fixtures first: those 30 bodies red, 50 clean bodies green; `WIP: Valid subject` green, `WIP` alone red |
| **1. Rounds in shadow** (#668) | lens briefs; refuters; round marker; `pr-lifecycle` by prompt; comments with `mode=shadow` | replay on the labelled set: #334-#364 (bad), #366 (revert), the 23 closed unmerged, and 40 merged-and-kept | the known-bad set is flagged high, the kept set stays mostly clean; the thresholds are recorded as data |
| **2. Duplicates** (#669) | the intake duplicate check | the compaction cluster and the sweep cluster each flagged against their siblings | fixture pairs, red and green |
| **3. Fixer and delta** (#670) | arbiter fixer with walls; delta rounds; `/override` | the §4 walkthrough end to end on a planted bug | the fixer's wall refuses a hand edit to a baseline; a delta round catches a planted regression pushed after clean |
| **4. On the channel** (#679) | after seven-primitives stages 4 and 6: the subscription, the adapter's outward writes (comment, title), self-authored rounds from the channel | a PR opened from Claude Code is reviewed with no verb run | the poll loops in §7 are gone and the line count falls |
| **5. Blocking** (#680) | flip `mode=shadow` to `blocking` once stage 1's criteria hold on the live PRs too (owner: "Shadow, then block") | a merge refused until round 2 is clean | the calibration query reports precision on the labelled set |

## 9. Decisions log (owner, 2026-09-26, verbatim)

The request: "new prs opened automatically open as drafts. drafts need two rounds of review
which manifest as two comments with some metadata or other identifier. the comments can be
self authored. … the reviewer will check for things like overcomplexity, bugs, naming
issues, ways to reduce loc, whether the pr is necessary in the first place, checks for
regressions, performance drains, etc"

| question | answer |
| --- | --- |
| where the bot runs | "docs/plans/2026-09-24-seven-primitives.md is an open plan that includes creating channels. assume that plan will be built out first" |
| draft representation | "WIP prefix + bot strips" |
| the two rounds | "Review, fix, re-review" |
| fixer authority | "Auto-push to PR branch" |
| round 2 clean bar | "Severity tiers" |
| an unneeded or duplicate PR | "Block + owner override" |
| template additions | "Why needed", "Deleted / alternatives", "Risk and rollback", "Performance" |
| a push after a round | "Delta round" |
| rollout | "Shadow, then block" |
| fixer seat | "Fresh fixer always" |
| GitHub | "Forge only now" |

## 10. Open questions

1. Forgejo Actions' default `pull_request` types are `opened`, `synchronize` and `reopened`.
   Does stripping `WIP:` fire `edited`, and does the `title` job need `types:` to
   include it? PR #377: a body edit left the title run stale.
2. Does Forgejo 15 refuse a merge of a `WIP:` PR through the API, or only in the UI?
   If only the UI refuses it, `just pr merge` needs the same refusal.
3. The duplicate thresholds: `grid diff` overlap and window-hash matches. Set them from
   stage 2's fixture pairs.
4. Stacked PRs have a base other than main. A delta round and a duplicate check should
   compare against the base branch, and the stack's successor should not flag its
   predecessor as a duplicate.
5. A collaborator's PRs. Seven-primitives says "the task's owner wins". Is the PR author the
   owner of `/override` on their own PR, or only you?
6. The reviewer model. The rule is the cheapest that passes calibration, and the reader
   family must differ from the author's. Claude Code PRs come from Anthropic models, so the
   readers there come from another family.

## 11. D-rows owed

Numbered at landing from the next free row on main; check open PRs for collisions.

- The PR lifecycle: rounds as todos pinned to a head sha, the marker format, and
  self-authored rounds from allowed accounts.
- The `WIP: ` title amendment to `check_commit_style`.
- Severity tiers and the one routing verdict (a high finding blocks ready); this depends on
  the seven-primitives "judges may route" row.
- Fixer authority: auto-push to the PR branch under the stated walls, a `just ratchet`
  commit ahead of a growing fix, capped at three rounds.
- Template v2 and the filled check.
- The duplicate check.
- Shadow-to-blocking promotion criteria.

## 12. Not building

- **Auto-merge**, in any form. #334-#364.
- **The GitHub side.** The mirror is one-way; wait for a PR there.
- **Fixing through the authoring session's mailbox**: a cold fixer only.
- **Embeddings or a vector index for duplicates**: `Closes #N`, `grid diff` and window
  hashes first; add more only if stage 2's fixtures miss.
- **A lens framework**: a lens is one `BRIEFS` entry.
- **A bot database or state store**: the plan journal is the state and the forge comment is
  its view.
- **Scheduled re-review sweeps** of open PRs: a round runs on an event, never on a clock.
- **Reviewer assignment to people**: rounds replace it; the owner's merge is the human step.
