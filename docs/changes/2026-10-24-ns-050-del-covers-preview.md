---
version: 0.793.0
decisions: D499
issue: Closes #1486, Refs #1442
decision: the covered-write preview is deleted by D228's own reversible-via: plan/covers.rs, ToolAdapter's check slot and the plan tool's covers property go; Contract.covers stays in yi-types unread so frozen contracts that carry it decode to the same digest, and old pending covered contracts still start (revises D228) | owner: "deleting the ~3,850 flagged lines" (2026-10-23); done stays the one acceptance, and Bet 3 runs a row check at done and answer only | revert this whole change (its parent-commit diff)
---

DEL-covers-preview deletes the covered-write preview from section 13.1. Writes no longer run a running todo's checker or append a preview verdict to the tool result, and the plan tool no longer offers `covers` in its contract schema.

`Contract.covers` remains only for wire compatibility: old journals decode and retain their recorded contract digests, and a pending covered todo still starts with its cmd items running at done. Natural plan-tool input leaves the retired globs out with guidance to that effect. The feature ledger now names the legacy-start and no-preview regression journeys.
