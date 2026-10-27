---
issue: Closes #1486, Refs #1442
decision: the covered-write preview is deleted by D228's own reversible-via: plan/covers.rs, ToolAdapter's check slot and the plan tool's covers property go; Contract.covers stays in yi-types unread so frozen contracts that carry it decode to the same digest, and a new contract naming covers is refused at start (revises D228) | owner: "deleting the ~3,850 flagged lines" (2026-10-23); done stays the one acceptance, and Bet 3 runs a row check at done and answer only | restore plan/covers.rs, the tools.rs slot and the schema property from this change's parent commit
---

DEL-covers-preview deletes the covered-write preview from section 13.1. Writes no longer run a running todo's checker or append a preview verdict to the tool result, and the plan tool no longer offers `covers` in its contract schema.

`Contract.covers` remains only for wire compatibility: old journals decode and retain their recorded contract digests, while `start` refuses a new covered contract with a sentence saying the cmd items run at done. Natural plan-tool input leaves the retired globs out with the same guidance. The feature ledger now names the refusal and no-preview regression journeys.
