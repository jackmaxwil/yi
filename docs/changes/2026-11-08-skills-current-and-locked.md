---
version: 0.1012.0
cut: .ruler/skills/har/SKILL.md | - `overflow-checks = true` under `[profile.release]` buys a loud failure for a small cost. | it reads as free, and with `panic = "abort"` beside it in `[profile.dist]` an overflow becomes a process abort, which under a zero panic budget is a budget violation no lint can see; the replacement states the two settings as one decision
cut: none .ruler/skills/har-api/SKILL.md
cut: .ruler/skills/har-async/SKILL.md | `Stream` is the async `Iterator`. **As of 2026-08-19 it is still not in std**: `core::async_iter::AsyncIterator` is nightly-only behind `feature(async_iterator)`, tracking issue rust-lang/rust#79024. The trait you use is `futures_core::Stream`, re-exported by `futures` and `tokio-stream`. | a hard date rots on its own; the claim is restated as the tracking issue's state, which is what a reader can check
cut: .ruler/skills/har-concurrent/SKILL.md | RUSTFLAGS="--cfg loom" cargo test --release --test loom | loom setup had two owners; har-verify owns the ladder, so it owns the setup, and har-concurrent keeps only the ordering facts loom exists to check
cut: .ruler/skills/har-hot-path/SKILL.md | - `iter().copied()` for small `Copy` types, `chunks_exact` over `chunks` — both codegen better. | `<[T]>::as_chunks` has been stable since 1.88 and yields real arrays, so indexing inside a chunk is checked at compile time; recommending `chunks_exact` now costs a bounds check and a panic site
cut: .ruler/skills/har-hot-path/SKILL.md | - `println!` locks stdout per call. In a loop, lock once and `writeln!` into a `BufWriter`. Both, not one. | har-layout owns output discipline and states the same rule; this skill keeps the pointer and not the restatement
cut: .ruler/skills/har-io/SKILL.md | description: Correctness at the edges of a Rust process — durable file writes and fsync, atomic replace, database transactions and retry classes, monotonic vs wall clock, deadlines and clock anomalies, CSPRNG and entropy failure, UTF-8 vs graphemes, normalization and identifier comparison. Load when a write must survive a crash, when code reads a clock or an RNG, or when text is sliced, truncated, or compared. | a trigger that names durability first would not fire on the case that bites most often; byte-versus-character indexing applies on every string edit, so text leads the description now
cut: .ruler/skills/har-layout/SKILL.md | Measured on rust-analyzer: 3x faster test compile, 5x smaller artifacts, run time 20s to 13s. | three numbers with no source in the file, cited as measurement; the replacement states the counterweight a reader can act on instead, which is that a process-per-test runner removes the sequential half of the cost
cut: none .ruler/skills/har-supply/SKILL.md
cut: .ruler/skills/har-threat/SKILL.md | Write the boundary list into `SUMMARY.md` next to the architecture it describes. An undocumented boundary gets no owner and no review. | the filename was invented; the rule is one durable document named in the repo's own contributing guide, and har-layout now says that in the same words
cut: .ruler/skills/har-threat/SKILL.md | - Generate keys and nonces from an RNG bounded by `CryptoRng`, never merely `RngCore` — `RngCore` alone admits `SmallRng`, which is fast, reproducible, and predictable. In `rand` 0.9 `CryptoRng: RngCore`, so `R: CryptoRng` is the whole bound; on `rand` 0.8 write `R: RngCore + CryptoRng`. | the bound has changed twice since; rand 0.10 has `CryptoRng: Rng<Error = !> + TryCryptoRng` and `RngCore` is not in it at all, so the replacement gives all three forms and sends the reader to the pinned version's own page
cut: none .ruler/skills/har-target/SKILL.md
cut: none .ruler/skills/har-unsafe/SKILL.md
cut: none .ruler/skills/har-verify/SKILL.md
keep: .ruler/skills/har-supply/SKILL.md | - Pin the toolchain in `rust-toolchain.toml` — same compiler, same lints, same output for everyone. A toolchain bump is its own commit, with its own full suite run. | it is the only statement anywhere requiring a bump to be isolated, no .ruler rule and no guardrail says it, and this branch rewrites the file it lives in while relying on it
---
The twelve har* skills are current on Rust 1.99.1, recommend nothing this repo's
dependency law forbids, and a hand-edit of the copy under `.ruler/skills` now fails
the build.

Ten claims in them were wrong or stale, each checked against a primary source read on
2026-11-08. Valgrind was listed as working here and supports no arm64 Darwin at all,
so the deterministic instruction-count lane is a Linux job. A HashDoS quote was
attributed to hashbrown and belongs to foldhash. The rustc-hash algorithm claim had no
source. `cargo llvm-lines` had no install line and is three characters from
`cargo-llvm-cov`. The lld default is `x86_64-unknown-linux-gnu`, not everywhere.
`cargo-witness` was cited as an established tool and 404s on docs.rs, so the
artifact-to-source gate is now recorded as having no mature tool, with the hand-diff
that stands in for one. The `rand` bound was two majors out of date. har-threat's
`frame()` did not compile: both `take` calls failed with E0507, not the E0277 this file
first recorded, and the first fix added the error enum the `?` sites needed while leaving
the `take` calls untouched. The block now imports `std::io::Read`, which is the whole
defect — with `BufRead` alone `take` resolves through the supertrait and moves the reader,
and `by_ref()` does not rescue it. Hand-checked on the pinned 1.94.0 in a scratch crate,
including a drain that resynchronizes and one that exhausts `MAX_DRAIN` without a newline.
Nothing in this repo compiles the rust blocks under `.ruler/skills`, so that check is not
repeatable here and the snippet can regress with every gate green; a gate that builds them
is worth its own branch.

Nineteen recommendations contradicted YI_DESIGN.md section 18.3 or deny.toml by name.
Every crate recommendation is now alternative-first: the dependency-free route leads,
the crate follows with its cost, and where no dependency-free route exists the skill
says so. That changes real advice in five places — har-layout's observability section,
which now opens with a dependency-free stderr logger and states that spans are the one
thing with no hand-rolled equivalent; har-hot-path's hashing, which leads with
`BuildHasherDefault`; `static_assertions`, now a const assert; `CachePadded`, now
`#[repr(align(128))]`; and har-threat's `subtle` requirement, which gained a
written-out constant-time compare and the statement that Rust guarantees no constant
time at the language level.

Four gaps are closed, each a kind of code written here: flat storage and generational
indexes, with the stale-index failure under a zero panic budget; a SIMD crate table
built on who picks the instruction set and when; build profiles per package; and
har-verify's case for Kani proving a panic budget in a crate that forbids `unsafe`,
which is the rung this codebase most needs and least reaches for.

The new gate locks a sha256 per skill rather than diffing against the canonical
source, because that repository is not checked out in CI. The copy stays a copy and
not a symlink, because `just install-skills` is `cp -R`.
