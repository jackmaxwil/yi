---
version: 0.694.0
issue: Closes #1422
---
A findings fix that follows one the review did not clear now runs at least one tier above it, never below. Before, the tier was scored afresh on each round. On #1415 and #1416, an Opus fix was followed by a GLM one, scored on a smaller round, which left the Opus findings standing or undid its design. The stop that compared held findings between rounds is gone: `holding()` reshapes what it holds by round number, so the counts jumped while a fix was doing its work. The fixer now stops at four fixes in a row, under the existing $8 per-PR cap.
