---
name: har-verify
description: Verification ladder for Rust — unit and property tests, cargo-fuzz, differential harnesses, miri, loom, kani proofs; when each rung pays and the minimal setup for each. Load when deciding how to test or prove a Rust change.
---

# Verification ladder

Dynamic testing proves a bug is **present**. Only the type system or a proof shows a bug class is **absent**. Climb until the rung matches the risk, then stop.

| Rung | Tool | Buys | Worth it when |
| --- | --- | --- | --- |
| 0 | `rustc` + `clippy` | memory safety, type errors, lint-level bugs | always, every build |
| 1 | `cargo test` vs ground truth | correctness on known inputs | always |
| 2 | doc tests | examples that cannot rot | any public API |
| 3 | `proptest` | invariants over generated input | function has an algebraic property |
| 4 | `cargo fuzz` | crash/panic hunting on untrusted bytes | code parses or decodes external input |
| 5 | differential harness | correctness vs a reference, at scale | reimplementing something with a trusted reference |
| 6 | `miri` | UB inside `unsafe`/FFI | crate or hot dependency has `unsafe` |
| 7 | `loom` | exhaustive interleaving and memory-ordering exploration | you own a lock, channel, or lock-free structure |
| 8 | `kani` | absence of panic/overflow/OOB, all inputs in bound | small, security-critical, self-contained fn |

## Rung 1: tests against ground truth

A round-trip test proves reversibility, not correctness — encrypt-then-decrypt passes on a backdoored cipher. Get external truth into `tests/`: spec vectors, RFC tables, recorded reference output.

Coverage is not state space: 100% line coverage still misses the input that trips one branch condition. Never make coverage the stopping rule. Every crash a fuzzer or user finds becomes a named test in `tests/` **before** the fix lands.

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

## Rung 4: fuzzing

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

Any panic, abort, OOM, or timeout is a finding. Set `overflow-checks = true` in the fuzz profile so silent wrap becomes a crash. Commit the corpus; commit each crash artifact as a regression test. Run timeboxed in CI (5 min) and long-form nightly.

## Rung 5: differential testing

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
cargo +nightly miri test -Zmiri-many-seeds
```

Detects out-of-bounds, use-after-free, misaligned access, uninitialized reads, data races, and aliasing violations — all of which need `unsafe` to introduce. Pointless on a `forbid(unsafe_code)` crate with no `unsafe` deps, valuable the moment either appears. 10-100x slow, so target a test subset. Cannot execute real FFI calls.

Those detections are exactly the UB classes it targets — data races, dangling or misaligned access, out-of-bounds projection, aliasing violations, and invalid values — which makes it the right rung the moment an `unsafe`-using dependency sits on a hot path. It models no FFI at all, so a C-backed crate is review plus the C side's own sanitizers. `-Zmiri-many-seeds` re-runs each test under many schedules, which is how weak-memory and interleaving bugs surface.

## Rung 7: loom

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

Run it with `RUSTFLAGS="--cfg loom" cargo test --release`. Inside `loom::model`, `loom::sync` and `loom::thread` replace their `std` counterparts; loom then runs every legal interleaving and every permitted ordering. State explodes with model size — two threads, two operations. x86-64 compiles `Relaxed`, `Acquire`, and `Release` loads and stores identically, so a memory-ordering bug is invisible there: run the concurrency test subset on aarch64, or under loom. `har-concurrent` owns the ordering rules themselves.

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

Proves no panic, overflow, or out-of-bounds access for **every** input inside the stated bound. Cost: loops need unwind bounds, state explosion limits input size, proofs need maintenance. Reserve for parsers, arithmetic on untrusted values, and any `unsafe` block.

## CI order

Fail fast, cheapest first:

```
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo deny check && cargo audit
cargo fuzz run <target> -- -max_total_time=300
```
