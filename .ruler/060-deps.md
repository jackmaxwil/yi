# Dependencies

A crate absent from YI_DESIGN.md §18.3 cannot be added. Adding one edits, in the same commit:
the §18.3 table (reason + alternative considered), deny.toml where relevant, and
docs/size-ledger.md with the measured dist-binary and startup delta. The §18.5 banned list
(reqwest, hyper, clap, anyhow, chrono, gix, once_cell, rand, toml, any *-sys, …) is
enforced by deny.toml. default-features = false everywhere; enable the minimum. Banned crates
inside a wrapped dependency's own tree (rmcp→chrono, zeromq→rand) get a scoped deny.toml
wrapper exception, never a repo-wide unban. `regex` left the banned list for grep v2 and is a
direct dependency; `syntect` left it for the highlighter, on `regex-fancy` only. Budgets are
whatever baselines/deps_budget.json currently reads; dist binary ≤ 6 MiB, `yi --version` ≤ 5 ms.
