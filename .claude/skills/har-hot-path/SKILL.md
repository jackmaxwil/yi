---
name: har-hot-path
description: Rust runtime performance — measure before changing, profiler and benchmark commands, allocation and clone rules, buffer reuse, type sizes, hashing, IO buffering, inline and release-profile tuning. Load before optimizing Rust or writing a frame, render, or parse loop.
---

# Hot paths

## Measure first

1. Reproduce on a realistic workload. Microbenchmarks supplement a decision; they never make it.
2. Never profile or benchmark a debug build. 10-100x differences are routine and they are not your code.
3. Wall time is what users feel but swings on memory layout alone. For A/B, instruction and cycle counts have far lower variance.
4. A mediocre measurement beats none. Do not build the perfect harness before starting.

A change with no before/after number is not an optimization, it is a rewrite.

## Profiler prerequisites

```toml
[profile.release]
debug = "line-tables-only"
```

Symbols for the profiler, no size explosion. Add `RUSTFLAGS="-C force-frame-pointers=yes"` when stacks are truncated, and `-C symbol-mangling-version=v0` when names are unreadable.

## Tools

| Tool | Command | Answers |
| --- | --- | --- |
| samply | `cargo install samply && samply record ./target/release/app` | where wall time goes; opens Firefox Profiler |
| flamegraph | `cargo install flamegraph && cargo flamegraph --bin app -- args` | one flame graph; needs `perf` (linux) or `dtrace` (macOS, sudo) |
| perf | `perf record -g ./target/release/app && perf report` | hardware counters, linux only |
| Instruments | `xcrun xctrace record --template 'Time Profiler' --launch -- ./target/release/app` | mac-native CPU, allocations, GPU and frame timing |
| hyperfine | `hyperfine --warmup 3 './old' './new'` | end-to-end wall time A/B with statistics |
| criterion | `cargo bench` with the `criterion` dev-dependency | per-function regression tracking, confidence intervals |
| divan | `cargo bench` with the `divan` dev-dependency | same job, lighter to write |
| callgrind | `valgrind --tool=callgrind ./app && callgrind_annotate callgrind.out.*` | per-line instruction counts, near-zero variance |
| DHAT | `valgrind --tool=dhat ./app` | which call sites allocate, and hot `memcpy` |
| cargo-bloat | `cargo bloat --release --crates` | what is actually in the binary |
| cargo-llvm-lines | `cargo llvm-lines \| head -20` | monomorphization bloat, the icache offender |
| type sizes | `RUSTFLAGS=-Zprint-type-sizes cargo +nightly build --release` | layout, padding, fat enum variants |

Profile first, benchmark second: the profiler says *where*, the benchmark says *whether it moved*.

## Allocation

- Pre-size anything whose final length you can estimate. `Vec::with_capacity(n)` turns four allocations into one for a twenty-element push loop.
- Declare the buffer outside the loop and `clear()` it per iteration. `clear` keeps capacity; a fresh `Vec::new()` does not.
- `read_line(&mut line)` plus `line.clear()` instead of `.lines()`, which allocates a `String` per line.
- `vec![0; n]` beats `resize`/`extend` — the OS hands back zero pages.
- `swap_remove` is O(1) where `remove` is O(n). `retain` for bulk removal.
- `ok_or_else` / `unwrap_or_else` / `map_or_else` whenever the default costs anything; `ok_or(build())` always evaluates.

## Clones

Cloning to silence the borrow checker is the defect. A deliberate clone in a cold path is fine; the pattern to kill is one the compiler talked you into.

| Situation | Instead of | Use |
| --- | --- | --- |
| Overwrite a vec from another | `dst = src.clone()` | `dst.clone_from(&src)` — reuses `dst`'s allocation |
| Shared immutable payload | `String` / `Vec<T>` clone | `Arc<str>` / `Arc<[T]>` — clone is a refcount bump, and one word smaller |
| Mutate a shared value if unshared | clone-then-mutate | `Rc::make_mut` / `Arc::make_mut` — copies only when refcount > 1 |
| Move a value out of a field | clone the field | `mem::take` / `mem::replace` |
| Two owners of mutable state | clone both sides | restructure, or `Rc`/`Arc` once |

## Buffers

| Situation | Do |
| --- | --- |
| Length known or estimable | `Vec::with_capacity(n)` |
| Rebuilt every iteration or frame | hoist out, `clear()` per pass |
| Usually at most N elements, N small | `SmallVec<[T; N]>` |
| Hard maximum known | `ArrayVec<[T; N]>` |
| Finished growing, stored many times | `into_boxed_slice()` — 2 words instead of 3, and it states the invariant |
| Result only ever iterated | return `impl Iterator<Item = T>`, skip the `Vec` |

## Strings and slices

`as_` is free, `to_` allocates, `into_` consumes. The prefix is a cost contract; `har-api` owns the naming rule, this is what it buys you at runtime.

| Situation | Take or return | Why |
| --- | --- | --- |
| Parameter, read-only | `&str` | zero cost, no monomorphized copies |
| Parameter, callee stores it | `String` | one clone, at the caller's choice, visible |
| Return, borrowed from an input | `&str` | free |
| Return, transform usually a no-op | `Cow<'a, str>` | zero allocations in the common case |
| Shared, immutable, crosses threads | `Arc<str>` | refcount clone, smaller than `String` |
| Encoding irrelevant | `&[u8]` | every `&str` construction pays UTF-8 validation |

Reach for `&[u8]` rather than an unchecked conversion; `har-unsafe` owns any unsafe fast path.

## Iterators

- One terminal `collect()`. Intermediate ones allocate for nothing.
- `filter_map` over `filter().map()`. `extend` into an existing collection over collect-then-append.
- `iter().copied()` for small `Copy` types, `chunks_exact` over `chunks` — both codegen better.
- `for x in &xs`, never `for i in 0..xs.len()`. Indexed access keeps the bounds check the iterator elides.
- Implement `size_hint` (or `ExactSizeIterator`) on custom iterators so `collect` and `extend` pre-allocate.

## Type sizes

`-Zprint-type-sizes` is the only honest source. The compiler already reorders fields, so hand-ordering is noise unless `#[repr(C)]`.

Box the rare fat variant — it shrinks every value of the enum, common variants included:

```rust
enum Node {
    Leaf(u32),
    Branch(Box<(Node, Node, Metadata)>),
}
```

Freeze a hot type so a later field addition fails the build:

```rust
static_assertions::assert_eq_size!(FrameSlot, [u8; 64]);
```

## Hashing

The default hasher is SipHash 1-3: strong, and slow on integer and short keys. `rustc_hash::FxHashMap` for internal maps keyed by ids where HashDoS is not in the threat model — rustc measured up to 6% total speedup on the switch, while `ahash` was 1-4% *slower* there. Measure; do not assume the ranking transfers. Keep the default hasher on any map keyed by attacker-supplied data.

## IO

- `println!` locks stdout per call. In a loop, lock once and `writeln!` into a `BufWriter`. Both, not one.
- Files are unbuffered. `BufReader`/`BufWriter` around anything you touch more than once.
- Call `flush()` explicitly. The flush in `Drop` swallows its error.

## Codegen cost

A generic function is compiled once per instantiation: build time, binary size, and instruction cache all pay. `cargo llvm-lines | head -20` names the offenders, `cargo bloat --release --crates` says what shipped.

```rust
pub fn read<P: AsRef<Path>>(path: P) -> Result<String, Error> {
    fn inner(path: &Path) -> Result<String, Error> { Ok(fs::read_to_string(path)?) }
    inner(path.as_ref())
}
```

The generic shell is trivial; the body exists once. Use `&dyn Trait` instead of `impl Trait` at a crate boundary where every downstream crate would otherwise monomorphize its own copy — one indirect call is cheaper than a cold icache.

## `#[inline]`

- Private functions: never. The compiler already sees the body.
- Generic functions: never. The body is already exported for instantiation.
- Small non-generic public functions callers should inline across a crate boundary — `Deref`, `AsRef`, accessors: yes. Or enable `lto`, which does it globally.

In an application, add it reactively after a profile points at the call. Sprinkling it costs build time and buys nothing.

## Release-profile tuning

`har-supply` owns the profile as policy; these are the runtime tradeoffs behind each knob.

| Knob | Buys | Costs |
| --- | --- | --- |
| `lto = "thin"` | 10-20%, cross-crate inlining | modest link time |
| `lto = "fat"` | more of the same | much slower builds |
| `codegen-units = 1` | more inlining within a crate | loses build parallelism |
| `panic = "abort"` | smaller binary, no unwind tables | no `catch_unwind`, no unwind-based cleanup |
| `-C target-cpu=native` | AVX and friends | binary only runs on that microarchitecture |
| `mimalloc` / `jemalloc` | large on allocation-heavy programs | one dependency, one line — measure it |
| PGO | around 10% | a profile-collection step in the build |

`overflow-checks = true` is real time in hot arithmetic, and `har-supply` sets it deliberately. Do not turn it off globally to win a benchmark: pick the explicit operation for the hot expression instead — `har` covers `checked_*` / `wrapping_*` / `saturating_*` — and keep the check everywhere else.

## The 120fps checklist

8.3 ms per frame. Steady-state paint:

1. Zero allocations. Every per-frame buffer is a field on the renderer, `clear()`ed at frame start, never recreated. Growth amortizes to zero within a few frames.
2. No `format!`. Cache formatted text in the model and invalidate on change; if text must be built per frame, `write!` into a reused `String`.
3. No `collect()`. Iterate. If a collection is unavoidable, it is a hoisted one.
4. Every clone is a refcount bump. `Arc<str>` / `Arc<[T]>` for shared frame data. A `String` or `Vec` clone per element per frame is the classic killer.
5. `FxHashMap` for id-keyed lookups, kept allocated across frames.
6. Hot structs stay small, and `assert_eq_size!` says so — layout regressions are otherwise invisible.
7. Anything unbounded — parse, IO, database — runs on a worker. The paint path reads a prepared snapshot.
8. No per-frame log line or span. Sample, or instrument the interaction instead: 120 formatted lines a second is cost and noise.

## Anti-patterns

| Anti-pattern | Fix |
| --- | --- |
| `String::new()` / `Vec::new()` / `format!` inside a loop | hoist and `clear()`, or `write!` into a reused buffer |
| `.lines()` over a large file | `read_line` into one reused `String` |
| `println!` per iteration | lock stdout once, `writeln!` into a `BufWriter`, explicit `flush` |
| Unbuffered `File` reads and writes | `BufReader` / `BufWriter` |
| Chained `collect()`s | one terminal `collect`, or `extend`, or return `impl Iterator` |
| `for i in 0..v.len() { v[i] }` | `for x in &v` |
| `ok_or(expensive())`, `unwrap_or(build())` | the `_else` variants |
| `v = other.clone()` | `v.clone_from(&other)` |
| `.clone()` added because the borrow checker complained | find the real conflict; `Arc`, `mem::take`, or restructure |
| Large generic body instantiated everywhere | thin generic wrapper delegating to a non-generic `inner` |
| `#[inline]` sprinkled across a module | delete; keep it on small public non-generic fns, or turn on `lto` |
| Default hasher on an internal id-keyed map | `FxHashMap` |
| Optimizing against a debug build or a microbenchmark alone | realistic workload, release build, a profiler |
