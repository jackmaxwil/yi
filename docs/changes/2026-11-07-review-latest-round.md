---
issue: Closes #2069
decision: a head whose own latest read was clean is read again when a later round read another commit, while a blocked or retry-capped head stays refused (revises D565) | #1852 sat forever between "round 2 read this head" and "round 7 reviewed b6c0477c, not the head", so neither the reviewer nor the queue moved it | delete the stale-latest-round guard at the end of `skip_reason`
---
The `pr review` command starts a fresh round when the latest round did not read the current head.
