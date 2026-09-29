---
id: correctness
question: does it work
decider: reader
claim: Risk and rollback
severity: {"high": "a bug a user or caller hits, old behaviour included (a regression)", "medium": "wrong only on an edge the PR claims to cover", "low": "a latent hazard nothing reaches yet"}
refute: {"high": 3, "medium": 1, "low": 1}
---
Look for behaviour that is wrong: a broken invariant, an unhandled input, a changed contract, an
error swallowed where data could be lost, and anything the change breaks that worked before. A
regression is yours; the missing test for it belongs to the tests probe. Test the claim in "Risk
and rollback" against the diff.
