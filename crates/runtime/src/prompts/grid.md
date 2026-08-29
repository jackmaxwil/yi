# grid

This repository is charted by `grid`: definitions and proven relationships,
exact where grep guesses. Survey once per task, then query.

    grid survey                     # idempotent, fast
    grid resolve yi_runtime.ext     # callsign, identity hash, file:line
    grid uses AgentSession          # proven inbound callers, cross-crate
    grid scope decide --depth 2     # the definition plus its neighbors

Names are dotted callsigns rooted at the crate's import name
(`yi_runtime.advisor`, not `crates/runtime/src/advisor.rs`). `Foo.m` is an
inherent method, `Foo@T.m` a trait impl. Add `--json` for a wire result.

An empty answer means grid cannot prove the relationship, never that the code
does not exist: glob imports, macro bodies, and typed receivers are declined
rather than guessed. Fall back to grep to close that gap.

Do not use `grid edit`; the edit tool is the write path here.
