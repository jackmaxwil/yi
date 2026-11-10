---
name: har-verify
description: Verification ladder for Rust — unit, snapshot and property tests, coverage as diagnostics, mutation adequacy, sanitizers and cargo-careful, cargo-fuzz, differential harnesses, miri, shuttle and loom, kani proofs, deductive proof tools, and the CI shape that turns them into evidence. Load when deciding how to test or prove a Rust change.
---

# Verification ladder

Dynamic testing proves a bug is **present**. Only the type system or a proof shows a bug class is **absent**. Climb until the rung matches the risk, then stop.

Rungs are ordered by cost, not by strength, and a higher one never supersedes a lower one: input generation, schedule exploration, UB detection and proof each establish a different claim over a different scope. Every gate you pass records what it actually proved — the property, the scope, the configuration or bound, the target, the corpus or seed, and the result.

| Rung | Tool | Buys | Worth it when |
| --- | --- | --- | --- |
| 0 | `rustc` + `clippy` | memory safety, type errors, lint-level bugs | always, every build |
| 1 | `cargo test` vs ground truth; snapshot tests | correctness on known inputs; regression on stable output | always |
| 1.5 | `cargo llvm-cov` | *which paths never ran* | as diagnostics, to aim the next test — never as a stopping rule |
| 2 | doc tests | examples that cannot rot | any public API |
| 3 | `proptest`, `bolero` | invariants over generated input | function has an algebraic property |
| 3.5 | `cargo-mutants` | whether the tests can tell a defect from correct code | after rungs 1-3 exist, on the modules that matter |
| 4 | `cargo-careful`, ASan / TSan | cheap native UB and race detection, FFI included | before spending time on fuzzing or miri |
| 5 | `cargo fuzz` | crash/panic hunting on untrusted bytes | code parses or decodes external input |
| 5.5 | differential harness | correctness vs a reference, at scale | reimplementing something with a trusted reference |
| 6 | `miri` | UB inside `unsafe`/FFI, on the paths your tests run | crate or hot dependency has `unsafe` |
| 7 | `shuttle`, then `loom` | schedule exploration; exhaustive interleaving on a small model | you own a lock, channel, or lock-free structure |
| 8 | `kani` | absence of panic/overflow/OOB for all inputs **inside a stated bound** | small, security-critical, self-contained sequential fn |
| 9 | `creusot`, `verus` | functional correctness from contracts and invariants | the few algorithms where "no panic" is not the property that matters |

Coverage is a side lane, not a rung, and never a release criterion. Concurrency (rung 7) is a second side lane: it answers a question no other rung asks.

Two of those rungs cost a **dependency** and the rest cost only an installed subcommand, which is a much smaller decision: `proptest`, `bolero`, `insta`, `loom` and `shuttle` link into your crate and go through `har-supply`'s admission; `cargo-mutants`, `cargo-careful`, `cargo-fuzz`, `kani`, `miri` and `cargo-llvm-cov` are tools that never appear in your graph. Note that a dependency gate configured to exclude dev-dependencies will not see the first group at all, so that admission is a human one.

## Rung 1: tests against ground truth

A round-trip test proves reversibility, not correctness — encrypt-then-decrypt passes on a backdoored cipher. Get external truth into `tests/`: spec vectors, RFC tables, recorded reference output.

Coverage is not state space: 100% line coverage still misses the input that trips one branch condition. Never make coverage the stopping rule. Every crash a fuzzer or user finds becomes a named test in `tests/` **before** the fix lands.

**Snapshot tests** are the golden-oracle form of this rung, for output that is complex, externally visible and meant to be stable: serialized wire messages, rendered diagnostics, an AST dump. `insta` is the standard tool. Two rules make them evidence rather than a rubber stamp: every changed snapshot is reviewed by a human as part of the diff, and CI runs in a mode that will not write or accept a new snapshot on its own. They catch regressions; they assert nothing semantic, so they never replace a ground-truth or differential test.

**`cargo-nextest`** is a better runner — process-per-test isolation, real parallelism, partitioning, machine-readable reports — and proves nothing extra. Adopt it for speed and failure isolation, and keep an explicit `cargo test --doc` gate, because nextest does not run doctests.

`assert_matches!` and `debug_assert_matches!` (std, 1.96) assert a value matches a pattern and print the value when it does not — the assertion to reach for on an enum, where `assert_eq!` would force a `PartialEq` impl you do not otherwise want and a `matches!` inside `assert!` throws the value away on failure.

## Rung 1.5: coverage as diagnostics

```
cargo install cargo-llvm-cov
cargo llvm-cov --workspace --all-features --lcov --output-path lcov.info
```

Read it to find the paths no test executes, then write those tests or seed the fuzzer with them. Publish the delta per PR and justify exclusions. Its branch and MC/DC reporting are nightly features — do not present that output as qualified certification evidence.

## Rung 3: property tests

```toml
[dev-dependencies]
proptest = "1"
```

```rust
proptest! {
    #[test]
    fn insert_then_get(k in any::<u32>(), v in any::<u64>()) {
        let mut m = Map::new();
        m.insert(k, v);
        prop_assert_eq!(m.get(&k), Some(&v));
    }
}
```

Good properties: round-trip (`decode(encode(x)) == x`), idempotence, ordering preserved, length/sum invariant, never-panics. Shrinking gives you the minimal failing input for free. Bad property: restating the implementation.

`bolero` sits at the same rung and runs **one** property under both a property-test engine and a fuzzing engine from a single harness; use it where you would otherwise write the same invariant twice. Neither tool is exhaustive.

Proptest persists a failing case and replays it on the next run — but only if you **commit the regression file**. Every generative failure, from any rung, ends as a minimized input in ordinary regression tests or a committed corpus, with its seed and configuration.

## Rung 3.5: mutation adequacy

```
cargo install cargo-mutants
cargo mutants --in-place --package parser
```

It edits the code in small ways and asks whether any test notices. A surviving mutant means the oracle may not distinguish a realistic defect from correct behaviour — it is not itself a defect, and a kill-rate percentage is not a release gate. Expensive, so run it on the safety-critical modules and on changed code, not the whole workspace on every PR. Gate on **triage of every relevant survivor** — real gap, dead code, equivalent mutant, or timeout — and fix gaps by testing public behaviour, not by asserting against the specific mutation.

**The cheapest mutant is one you write, and for a control it is the only one that works.** Where the thing under test is a *control* rather than a computation — a guardrail script, a permission check, a resource bound — the mutation is disabling the control and asserting the test fails for that control's own stated reason. That is `har-threat`'s "test the control, not the feature" made executable, and a generic mutation tool cannot do it because the control's logic is policy, not arithmetic. Build it into the gate as a `--selfcheck` flag that disables each check in turn and requires each to fail for its own reason, so the refute pass runs on every invocation instead of once by hand when the gate was written.

## Rung 4: sanitizers and cargo-careful

Cheaper than fuzzing or miri, and they see things miri cannot because they run real native code, FFI included.

```
cargo install cargo-careful
cargo +nightly careful test
```

`cargo-careful` rebuilds `std` with debug assertions and extra checks: faster and far less exhaustive than miri, and it needs a recent nightly plus `rustc-src`.

Sanitizers go in their own jobs, never combined:

```
RUSTFLAGS="-Zsanitizer=address" cargo +nightly test --target x86_64-unknown-linux-gnu
RUSTFLAGS="-Zsanitizer=thread"  cargo +nightly test --target x86_64-unknown-linux-gnu
```

ASan/LSan for native memory and lifetime bugs, TSan as a separate concurrency job, MSan only where every dependency and `std` can be instrumented. The sanitizer interface is nightly and target-dependent, so pin the toolchain, target and `RUSTFLAGS` per job and record them. All of these are dynamic bug detectors; none is a proof.

## Rung 5: fuzzing

```
cargo install cargo-fuzz
cargo fuzz init
cargo fuzz add parse
cargo fuzz run parse -- -max_total_time=300 -jobs=8
```

```rust
fuzz_target!(|data: &[u8]| { let _ = parse(data); });

#[derive(Arbitrary, Debug)]                      // structured input: fuzz the type, not bytes
enum Op { Insert(u32, u64), Remove(u32), Get(u32) }
fuzz_target!(|ops: Vec<Op>| { replay(ops); });
```

Any panic, abort, OOM, or timeout is a finding. Set `overflow-checks = true` in the fuzz profile so silent wrap becomes a crash.

`cargo-fuzz` is a libFuzzer driver — nightly, a C++11 compiler, and an LLVM-sanitizer-supported platform. **One target for five minutes is not a fuzz gate.** Maintain an inventory of fuzz targets — every parser, decoder and unsafe boundary, per feature configuration — and split the work: a PR job replays the committed corpus deterministically and smoke-runs each target briefly; a scheduled job runs long campaigns. Commit seeds and the minimized corpus; triage every artifact into a named regression test. Use coverage to aim corpus and harness work, not to declare the target done.

## Rung 5.5: differential testing

Run your implementation and a trusted reference over the same generated input; assert equal results at every step.

```rust
fuzz_target!(|ops: Vec<Op>| {
    let (mut mine, mut reference) = (MyMap::new(), std::collections::BTreeMap::new());
    for op in ops {
        assert_eq!(apply_mine(&mut mine, &op), apply_ref(&mut reference, &op));
    }
    assert!(mine.iter().eq(reference.iter()));
});
```

Catches semantic divergence a panic-hunting fuzzer never sees. References: the std collection you replaced, the C library you ported, another crate on the same spec. Compare final state, not just return values.

## Rung 6: miri

```
rustup +nightly component add miri
cargo +nightly miri test
MIRIFLAGS="-Zmiri-many-seeds=0..64" cargo +nightly miri test
MIRIFLAGS="-Zmiri-tree-borrows" cargo +nightly miri test
```

Detects out-of-bounds, use-after-free, misaligned access, uninitialized reads, data races, and aliasing violations — all of which need `unsafe` to introduce. Pointless on a `forbid(unsafe_code)` crate with no `unsafe` *reachable* — and that second half is a question, not an assumption. Almost every real graph contains `unsafe`: a syscall wrapper, a TLS stack, a terminal emulator, a decoder. `cargo geiger` plus `cargo tree --invert` (`har-supply`) tell you whose, and whether your tests execute it. A RustSec **unsound** advisory against anything in your lockfile is this rung's exact bug class arriving pre-located, with the crate and the pattern already named — check for those before concluding the rung is not yours. 10-100x slow, so target a test subset: the safe wrappers around `unsafe`, the FFI adapters it can execute, and minimized fuzz and property regressions.

**Miri is a path-sensitive UB detector, not a soundness verdict.** It observes the executions your tests perform; it has no networking and incomplete system and FFI support; its weak-memory emulation is incomplete; and its aliasing model is experimental (`har-unsafe` owns Stacked vs Tree Borrows). A clean run closes no unsafe review and discharges no proof obligation, and never disable its validation or borrow checking to make an assurance run pass. `-Zmiri-many-seeds` re-runs each test under many schedules, which is how interleaving bugs surface.

## Rung 7: schedule exploration

Two tools, in this order.

**`shuttle`** randomizes schedules over ordinary `std` primitives, scales to realistic components, and reproduces a failure from its seed. Start here.

**`loom`** is exhaustive over a small model:

```toml
[target.'cfg(loom)'.dev-dependencies]
loom = "0.7"
```

```rust
#[test]
fn push_then_pop() {
    loom::model(|| {
        let q = loom::sync::Arc::new(Queue::new());
        let w = { let q = q.clone(); loom::thread::spawn(move || q.push(1)) };
        q.pop();
        w.join().unwrap();
    });
}
```

Run it with `RUSTFLAGS="--cfg loom" cargo test --release`. Inside `loom::model`, `loom::sync` and `loom::thread` replace their `std` counterparts. State explodes with model size — two threads, two operations. Loom explores only code written against **its own** primitives and does not implement the full C11 model: it treats `SeqCst` as `AcqRel` and skips some load-buffering outcomes. So a green loom run is bounded, model-specific evidence about the primitive you modelled — keep tests against the real primitives too.

x86-64 compiles `Relaxed`, `Acquire`, and `Release` loads and stores identically, so a memory-ordering bug is invisible there: run the concurrency subset on aarch64 as well. `har-concurrent` owns the ordering rules themselves.

## Rung 8: kani proofs

```
cargo install --locked kani-verifier && cargo kani setup
```

```rust
#[kani::proof]
#[kani::unwind(33)]
fn parse_never_panics() {
    let len: usize = kani::any();
    kani::assume(len <= 32);
    let buf = vec![kani::any::<u8>(); len];
    let _ = parse(&buf);
}
```

**The case nobody makes for Kani: proving a panic budget.** A crate that forbids `unsafe` has no UB for miri to find, which makes the ladder look like it stops at rung 5. But a no-panic policy enforced by linting on `unwrap`, `panic!`, `todo!` and friends is enforced on *spellings*, and the panics that matter do not spell themselves: `&s[..80]` landing mid-character, an `a + b` that overflows, a `chunks(0)`, a `Vec::remove` past the end, a `String::truncate` on a non-ASCII byte. Every one passes a spelling gate and panics at run time. Kani checks the behaviour, so one `#[kani::proof]` per function that slices, indexes or computes on an unvalidated value turns the policy into evidence inside the stated bound. That makes this the highest-value rung for precisely the codebase that believes it does not need rung 6.

Proves no panic, overflow, or out-of-bounds access for **every** input inside the stated bound. It is bounded model checking, and the bound is part of the result: the proof is sound only when every loop-unwinding assertion passes at the recorded bound. Kani is **sequential** — it detects neither data races nor pointer-aliasing violations — and does not support inline assembly. Reserve it for parsers, arithmetic on untrusted values, and small self-contained `unsafe`, and make every result carry its harness preconditions, unwind bound, enabled checks, and the list of unsupported operations you had to review by hand. Concurrent algorithms get rung 7 or rung 9, not this.

Beyond `#[kani::proof]` and `#[kani::unwind(n)]`, Kani takes **contracts**: `#[kani::requires]`, `#[kani::ensures]` and `#[kani::modifies]` on the function, verified through `#[kani::proof_for_contract(f)]`, plus loop contracts — `#[kani::loop_invariant]`, `#[kani::loop_modifies]`, `#[kani::loop_decreases]`. The loop contracts are the answer to the cost below: a loop invariant is how you stop paying an unwind bound for a loop whose bound is the thing you cannot state.

Cost: loops need unwind bounds, state explosion limits input size, proofs need maintenance.

## Rung 9: deductive proof

For the handful of algorithms, unsafe abstractions and protocol invariants where "does not panic" is not the property you care about, write contracts and loop invariants and prove them. None of these is drop-in verification of arbitrary Rust, and the honest state of the tools matters more than the list:

| Tool | Usable for | Caveat |
| --- | --- | --- |
| Creusot | contracts and invariants on safe Rust, discharged via Why3 | `cargo creusot prove`; specification effort is the real cost |
| Verus | systems code written for verification, including some unsafe | excludes or partially supports major features — async, most `std` sync |
| Flux | refinement types as a lightweight CI gate | nightly + Z3 prototype; its own docs warn it can break |
| Prusti | a restricted safe fragment | self-described prototype |
| Aeneas | translating a safe sequential subset to F\*/Coq/HOL4/Lean | unsafe and concurrency support still landing |

Pick the tool for a component you are willing to *write for verification*, pin its toolchain, and record every unsupported feature and trusted axiom as an explicit residual risk. The Rust project's own **Verify Rust Standard Library** effort is a useful model of that discipline — tool-specific harnesses, explicit scope — and is not a certification you can inherit.

## Safety-critical projects

If the product has a hazard domain, the governing standard is the domain's — ISO 26262 (automotive), IEC 61508 (industrial), DO-178C (airborne) — and it wants a product-specific safety case: named integrity level, requirements traced to tests and proofs, review independence, toolchain configuration control, tool qualification or justified tool confidence, and the coverage evidence the standard mandates. Ferrocene is a qualified toolchain, not a qualified program.

The Rust Project explicitly does not certify Rust; it is building foundations — FLS release tracking, safety-critical lints, unsafe-code guidance, MC/DC support — and positions those foundations around ASIL A/B and SIL 1/2, with higher integrity a target rather than a claim. So name the standard and integrity level at project start, and never present `cargo-llvm-cov` output as MC/DC certification evidence.

## CI

Three workflows, not one. Every job pins its toolchain and tool versions, builds `--locked`, and archives what it produced.

**PR, fast — fail cheapest first:**

```
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
cargo test --workspace --all-features --locked
cargo test --doc --workspace --locked
cargo deny check && cargo audit && cargo vet --locked
cargo fuzz run <target> -- -runs=0 -seed_inputs=fuzz/corpus/<target>
```

Bare `cargo clippy --all-targets` and bare `cargo test` cover **default members and default features only**, which silently skips `required-features` binaries and every non-default configuration. Name the scope explicitly, and add a matrix for `--no-default-features`, each supported feature set, the MSRV toolchain, the release profile, and every supported target.

**Nightly or periodic:** `cargo careful test`; ASan and TSan jobs; `cargo miri test` on the unsafe subset, plus a Tree Borrows run; `cargo mutants` on changed files; long `cargo fuzz` campaigns; the shuttle and loom suites; `cargo llvm-cov` with a published delta; an aarch64 test job.

**Release candidate:** the full target matrix, `cargo semver-checks` per supported feature configuration (`har-api` owns its limits), every kani proof with its bounds recorded, and the archived evidence set — coverage deltas, fuzz corpora and artifacts, proof bounds, sanitizer reports, and every exception with its approval.

A ladder in prose is not a gate. A rung is only real once a named job runs it and produces something a reviewer can read.
