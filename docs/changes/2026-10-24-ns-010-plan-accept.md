---
issue: Closes #1452, Refs #1442
raise: tests +319
decision: Verification runs up to max(1, min(4, slots/4)) acceptances at once and a waiter waits until the verifier's deadline | owner: "Fix bugs, keep design"; one permit with a 30 s wait bounced every concurrent worktree acceptance whose check outlasted it | restore VERIFICATION_RESERVE 1 and RESERVE_WAIT 30 s
---
A verification token, the `verification_requested` record and the `verification_stale` record now each have one constructor, called by the done path and by worktree acceptance alike. The done path decides staleness by comparing the token it built at the start with one it builds again at the end, so two builders that drifted apart would have refused a sound `done` as stale with nothing to say why. The contract-drift check and the refusal of a candidate or integration check are shared the same way.

Worktree acceptances verify side by side. The lane pool kept one verification lane whatever its size, and an acceptance waiting for it gave up after 30 s, so a second child whose check ran longer was bounced back to `candidate_submitted`, where `done` answered that a phase was missing and the child, already ended, could not submit again. A quarter of the pool now verifies, at least one lane and at most four (two in a pool of eight, four in the default pool), and a waiter waits until the session's deadline, or the verifier's own timeout when the session has none. Members: HA-HB-token, HA-HB-verify-cap.
