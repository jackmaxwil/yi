# Size ledger (§13.6)

Measured on macOS arm64, rustc 1.94.0, profile `dist`. Candidate-dependency deltas (ureq vs
reqwest, jiff vs chrono, globset vs regex-lite) are recorded here before each choice is final;
the §13.3 table is the expected outcome, this ledger wins if they disagree.

| date | change | dist binary (bytes) | `yi --version` (ms) | deps direct/transitive |
|---|---|---|---|---|
| 2026-08-22 | phase 0 scaffold: empty 13-crate workspace, no external deps | 286064 | 1.4 | 0 / 0 |
