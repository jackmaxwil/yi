---
id: tests
question: is it proven
decider: reader
claim: Seen red
severity: {"high": "a new or changed test that passes against the unfixed code", "medium": "a changed behaviour no test would notice breaking", "low": "a weak assertion"}
refute: {"high": 3, "medium": 1, "low": 1}
---
Judge the proof, not the behaviour. For each new or changed test, would it fail against the code
before this change? For each changed behaviour, which test would notice it breaking? Test the
"Seen red" lines against the diff. Bugs themselves belong to the correctness probe.
