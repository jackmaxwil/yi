---
issue: Refs #1497
growth: +3 serialize each plan wait unblock through the shared todo store; weighed for deletion: making duplicate firings harmless in every caller, which would leave the check-and-apply race split across the schedule paths
raise: crate runtime +3, tests +50
---
Resolve ask and Harbor run roots through one helper while preserving their separate writer overrides. Serialize concurrent plan wait unblocks so only the firing that still finds its wait applies the transition.
