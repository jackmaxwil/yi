---
id: perf
question: does it work
decider: reader
claim: Performance
when: {"paths": ["crates/*/src/*"]}
severity: {"high": "a measured or certain regression on a hot path", "medium": "a likely one"}
refute: {"high": 3, "medium": 1}
---
Look for performance drains on a hot path: work repeated per item or per frame, a blocking call on
an async path, an unbounded buffer, an allocation in a loop. Test the claim in "Performance".
