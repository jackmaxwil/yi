---
issue: Closes #1413
---
The autofixer no longer pays for a fix it cannot make better by climbing tiers. When the model answers every finding of a round with a reason and changes nothing, the fixer posts the declines once and waits for a new head. Before, it counted that as a miss and climbed a tier; #1387 paid three tiers to decline one stale finding and then failed. The fixer stops for the owner only when a declined finding is high. When the model edits a baseline under `scripts/guardrails/baselines/`, the fixer puts that file back and keeps the rest of the fix, since a baseline moves only by its gate's own `--update`. Before, the whole attempt was thrown away; #1405 lost its fix to one edit of `binary_size_budget.json`. Edits to the gates themselves are still refused.
