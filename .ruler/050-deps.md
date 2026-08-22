# Dependencies

A crate absent from YI_DESIGN.md §13.3 cannot be added. Adding one edits, in the same commit:
the §13.3 table (reason + alternative considered), deny.toml where relevant, and
docs/size-ledger.md with the measured dist-binary and startup delta. The §13.5 banned list
(reqwest, hyper, clap, anyhow, chrono, regex, gix, once_cell, rand, toml, any *-sys, …) is
enforced by deny.toml. default-features = false everywhere; enable the minimum. Budgets,
ratcheted: ≤ 16 direct, ≤ 110 transitive, dist binary ≤ 6 MiB, `yi --version` ≤ 5 ms.
