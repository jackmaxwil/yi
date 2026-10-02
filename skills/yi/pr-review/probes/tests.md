---
id: tests
question: is the proof real
decider: reader
claim: Seen red
when: {"paths": ["crates/*", "python/*", "scripts/*", "evals/*", "adapters/*"]}
severity: {"high": "a test the PR offers as proof (in Seen red, or as a bug fix's regression test) that would pass against the code before this change", "medium": "a behaviour a user or caller sees changes and no test, new or old, would fail if it broke"}
refute: {"high": 3, "medium": 1}
---
Judge only whether the PR's proof is real. A test the PR offers as proof must fail against the
code before the change; when it would pass, it proves nothing, and that is the finding. Name a
changed behaviour only when a user or caller would see it and no test anywhere would notice it
breaking. Do not ask for more unit tests, coverage of internal helpers, extra edge cases, stronger
assertions or another test style: none of those is a finding. Bugs belong to the correctness probe.
