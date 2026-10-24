---
issue: Closes #1452, Refs #1442
---
A verification token, the `verification_requested` record and the `verification_stale` record now each have one constructor, called by the done path and by worktree acceptance alike. The done path decides staleness by comparing the token it built at the start with one it builds again at the end, so two builders that drifted apart would have refused a sound `done` as stale with nothing to say why. The contract-drift check and the refusal of a candidate or integration check are shared the same way. Members: HA-HB-token.
