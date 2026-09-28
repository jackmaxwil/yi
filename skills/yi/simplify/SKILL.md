---
name: simplify
description: >
  A subtract-first pass over a diff before it lands: reuse what exists,
  delete what nothing reads, lower the altitude of a special case, refuse
  a new dependency. Use when the user says simplify, reduce, shrink, "is
  there a smaller way", or before landing a change over a hundred lines.
  Do NOT use to hunt bugs (that is a review) or on someone else's diff
  without being asked.
trigger: simplify, shrink this, smaller diff, subtract first
---

# simplify

`simplify` → `git diff` → four angles → the shortest working diff → the gate

## Phase 0: the diff

```
git diff @{upstream}...HEAD 2>/dev/null || git diff origin/main...HEAD || git diff HEAD
```

Read it whole. The diff is ground truth; the description of it is a
claim.

## The four angles

1. Reuse. For every new type, function or helper: does one already exist
   in this repository? `grep` for the name and the shape; `grid resolve`
   where the binary is present. Replace the new one with the existing one.
2. Dead code. What does nothing read? A field written and never read, a
   function with one caller that could inline it, an `Other` arm that
   cannot occur. Delete it; the orphan gate counts write-only fields.
3. Altitude. A special case layered on shared infrastructure says the fix
   is not deep enough. One guard where every caller routes through beats a
   guard per caller. Three similar lines beat a premature abstraction.
4. Dependency. A new crate for what a few lines do is refused
   (`deny.toml` and the §18.3 table decide). A feature flag exists only
   where the design declares it.

With `rlm.run` available and a diff over five hundred lines, give each
angle to a child with the diff and the angle's paragraph; without,
take the angles in order yourself.

## Apply

Make the cuts; the diff must still pass its focused tests and the gate
by exit code. Net negative lines is the default win; growth needs a
reason the report names.

## Never simplify away

Trust-boundary validation, error handling that prevents data loss,
security, accessibility, migration and rollback safety, concurrency
protection, anything explicitly requested. A repository rule that blocks
the smallest change is named in the report, not worked around.

## Report

Lines before and after, what each angle removed, and what was kept on
purpose with its reason.
