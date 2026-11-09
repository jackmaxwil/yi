---
name: har-hot-path
description: Rust runtime performance — measure before changing, profiler and benchmark commands, black_box, allocation and clone rules, arenas, buffer reuse, bounds-check elision, type sizes, hashing, inline and cold, release-profile and PGO tuning. Load before optimizing Rust or writing a frame, render, or parse loop.
---

# Hot paths

## Measure first

1. Reproduce on a realistic workload. Microbenchmarks supplement a decision; they never make it.
2. Never profile or benchmark a debug build. 10-100x differences are routine and they are not your code.
3. Wall time is what users feel but swings on memory layout alone. For A/B, instruction and cycle counts have far lower variance.
4. A mediocre measurement beats none. Do not build the perfect harness before starting.

A change with no before/after number is not an optimization, it is a rewrite.

## Build for profiling

```toml
[profile.profiling]
inherits = "release"
debug = "line-tables-only"
strip = false
```

Symbols for the profiler, no size explosion, and the shipped `release` profile stays stripped. Add `RUSTFLAGS="-C force-frame-pointers=yes"` when stacks are truncated, and `-C symbol-mangling-version=v0` when names are unreadable. On Linux, `cargo flamegraph` needs `--no-rosegment` when the binary was linked with `lld` — the default on `x86_64-unknown-linux-gnu` since 1.90, not on every target — or with `mold`, or the stacks are wrong.

## Tools

| Tool | Command | Answers | Platform |
| --- | --- | --- | --- |
| samply | `cargo install samply && samply record ./target/profiling/app` | where wall time goes; opens Firefox Profiler | all; on Linux it samples **on-CPU only**, on macOS both |
| perf | `perf record -g ./target/profiling/app && perf report` | sampling plus real PMU counters — the only honest source for cache and branch events | Linux |
| flamegraph | `cargo flamegraph --profile profiling --bin app -- args` | one flame graph | needs `perf` (Linux) or `dtrace` (macOS, sudo) |
| Instruments | `cargo instruments -t 'Time Profiler' --profile profiling` | mac-native CPU, allocations, GPU and frame timing | macOS |
| heaptrack | `heaptrack ./target/profiling/app` | allocation traces over time | Linux |
| DHAT | `valgrind --tool=dhat ./target/profiling/app` | which call sites allocate, and hot `memcpy` | Linux, x86-64 macOS ≤ 13 |
| cachegrind | `valgrind --tool=cachegrind ./app && cg_annotate cachegrind.out.*` | exact, reproducible instruction counts, flat — best for a small regression diff | Linux, x86-64 macOS ≤ 13 |
| callgrind | `valgrind --tool=callgrind ./app && callgrind_annotate callgrind.out.*` | the same counts plus caller/callee attribution | Linux, x86-64 macOS ≤ 13 |
| iai-callgrind | `cargo bench` with the `iai-callgrind` dev-dependency | deterministic counter benchmarks, low enough variance for CI | Linux, x86-64 macOS ≤ 13 |
| hyperfine | `hyperfine --warmup 3 './old' './new'` | end-to-end wall time A/B with statistics | all |
| criterion / divan | `cargo bench` with that dev-dependency | per-function regression tracking; divan is lighter to write | all |
| cargo-bloat | `cargo bloat --release --crates` | what is actually in the binary | all |
| cargo-llvm-lines | `cargo install --locked cargo-llvm-lines && cargo llvm-lines \| head -20` | monomorphization bloat, the icache offender | all |

**Everything Valgrind-based is Linux or an old x86-64 macOS.** Valgrind supports no arm64 Darwin at all — its platform list stops at `AMD64/Darwin (10.8 to 13.0)` — so on an Apple-silicon machine the deterministic instruction-count lane is a Linux CI job or a container, not a local run. `samply` plus Instruments is the native macOS pair. Note also that `cargo-llvm-lines` is a different tool from `cargo-llvm-cov` (`har-verify` rung 1.5); the names differ by three characters and the mistake is silent.

Cachegrind's and Callgrind's cache and branch simulation is a rough model, not the machine — read it as diagnostic, and use `perf stat` against the real PMU when cache or branch behaviour is the question.

**Nightly compiler diagnostics**, pinned like any other tool (`har-supply`), and not runtime measurements:

```
RUSTFLAGS=-Zprint-type-sizes cargo +nightly build --release   # layout, padding, fat enum variants
```

Profile first, benchmark second: the profiler says *where*, the benchmark says *whether it moved*.

## Benchmarks lie without `black_box`

Criterion, divan, a release build and instruction counts do nothing to stop LLVM from constant-folding a literal input, hoisting the work out of the loop, or deleting a pure result nobody reads. `std::hint::black_box` is the documented best-effort barrier, and it applies to **its own output**, so the input and the result are two separate calls:

```rust
b.iter(|| black_box(parse(black_box(&input))));
```

It is a benchmarking tool only. Never cite it for correctness, constant-time, or any security property.

## Allocation

- Pre-size anything whose final length you can estimate. `Vec::with_capacity(n)` turns four allocations into one for a twenty-element push loop.
- Declare the buffer outside the loop and `clear()` it per iteration. `clear` keeps capacity; a fresh `Vec::new()` does not.
- `read_line(&mut line)` plus `line.clear()` instead of `.lines()`, which allocates a `String` per line.
- `vec![0; n]` beats `resize`/`extend` — the OS hands back zero pages. `Box::new_zeroed_slice(n)` (1.92, with `Rc`/`Arc` variants) states that intent directly for a buffer that is never going to grow, and hands back `Box<[MaybeUninit<T>]>` so the "it is zeroed, therefore initialized" step is yours to make explicitly.
- `Vec::push_mut` / `insert_mut` (1.95, with the `VecDeque` and `LinkedList` forms) return `&mut` to the element just inserted, which removes the `last_mut().unwrap()` or the re-index that usually follows a push — and with it a panic site.
- `VecDeque::pop_front_if` / `pop_back_if` (1.93) and `BTreeMap`/`BTreeSet::extract_if` (1.91) do conditional removal in one pass instead of a scan plus a second structural edit.
- `dst.extend_from_slice(src)` to append a slice, not a hand-written `push` loop: it states the bulk-copy intent and `std` specializes it to `copy_from_slice` for `Copy` elements. Its bound is `T: Clone`, so `dst.extend(src.iter().copied())` where that suits better. `reserve` first when a larger composition makes the final length known.
- `swap_remove` is O(1) where `remove` is O(n). `retain` for bulk removal.
- `ok_or_else` / `unwrap_or_else` / `map_or_else` whenever the default costs anything; `ok_or(build())` always evaluates.

**Arenas**, when a profile shows many small heterogeneous allocations that all die together — a parser, an AST, a query plan, a per-request graph. A bump allocator (`bumpalo`) turns each allocation into a pointer bump and the whole phase into one reset. Two conditions: no reference may escape the phase, and nothing in the arena may need a prompt `Drop`, because a bump reset does not run destructors. Use a drop-aware wrapper or explicit cleanup where it does.

## Clones

Cloning to silence the borrow checker is the defect. A deliberate clone in a cold path is fine; the pattern to kill is one the compiler talked you into.

| Situation | Instead of | Use |
| --- | --- | --- |
| Overwrite a vec from another | `dst = src.clone()` | `dst.clone_from(&src)` — reuses `dst`'s allocation |
| Immutable payload, **uniquely owned**, fixed size | `String` / `Vec<T>` | `Box<str>` / `Box<[T]>` — 2 words, no refcount |
| Immutable payload **genuinely shared** across threads | `String` / `Vec<T>` clone | `Arc<str>` / `Arc<[T]>` — clone is a refcount bump |
| Immutable payload shared on one thread | `Arc` | `Rc` — no atomics |
| Mutate a shared value if unshared | clone-then-mutate | `Rc::make_mut` / `Arc::make_mut` — copies only when refcount > 1 |
| Move a value out of a field | clone the field | `mem::take` / `mem::replace` |
| Two owners of mutable state | clone both sides | restructure, or `Rc`/`Arc` once |

`Arc` is not the default shared-payload type — it costs an allocation and atomic refcount traffic on every clone and drop, and the Performance Book calls `Rc`/`Arc` expensive when sharing is rare. Reach for `Box<str>`/`Box<[T]>` when ownership is unique, borrow `&Arc<T>` rather than cloning it, and measure the contention when a hot `Arc` is cloned across threads. `Vec::into_boxed_slice` is a finalization step at the ownership boundary; it reallocates when `len < capacity`, so it is not free.

## Buffers

| Situation | Do |
| --- | --- |
| Length known or estimable | `Vec::with_capacity(n)` |
| Rebuilt every iteration or frame | hoist out, `clear()` per pass |
| Allocation profiling shows a small bounded common length | `SmallVec<[T; N]>` — **measured**, see below |
| Hard maximum known, no heap wanted | `ArrayVec<[T; N]>` |
| Finished growing, stored many times | `into_boxed_slice()` — 2 words instead of 3, and it states the invariant |
| Result only ever iterated | return `impl Iterator<Item = T>`, skip the `Vec` |

Those last two are crates, and the dependency-free forms come first: a hoisted `Vec` with `with_capacity` for the bounded-common-length case, and `[T; N]` plus a `len` field — or `[Option<T>; N]` — where the maximum is hard. `SmallVec` and `ArrayVec` reduce allocation rate and are not free defaults even once admitted: inline storage enlarges every enclosing object and adds a branch on each access, so inside a struct that appears a million times they can cost more cache than they save allocator. Adopt them when a profile shows the bounded common length **and** you have measured the enclosing object's size (`har-supply` owns the admission).

## Flat storage and generational indexes

When a graph or a pool needs identity without ownership, the shape is a flat `Vec` plus a newtype index — not `Rc<RefCell<T>>`, not a lifetime (`har-api` owns the borrow-checker ladder that gets you here). Preconditions: the collection is append-mostly or slot-reusing, the index is never arithmetic a caller can forge, and lookups outnumber structural edits.

**The index carries a generation, or it is unsound by reuse.** A bare index is A→B→A — the same hazard `har-concurrent` names ABA, reached with no atomics at all: free slot 7, allocate slot 7, and every id still naming 7 now addresses a different value. Under a zero-panic budget the resolve step cannot be `slots[i]` and cannot be `.expect("live")`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct NodeId { index: u32, generation: u32 }

struct Slot<T> { generation: u32, value: Option<T> }

impl<T> Arena<T> {
    pub fn get(&self, id: NodeId) -> Result<&T, Error> {
        let slot = self.slots.get(id.index as usize).ok_or(Error::BadIndex)?;
        if slot.generation != id.generation { return Err(Error::Stale); }
        slot.value.as_ref().ok_or(Error::Vacant)
    }
}
```

Private fields, `get` returning `Result`, and `Stale` as a variant a caller can act on. Bump the generation **on free, not on allocate**, and make it `checked_add` rather than wrap — a wrapped generation is the exact bug the field exists to prevent. The test that proves the field works is the one that reuses a slot and asserts the old id is `Err(Stale)` rather than the new value; without it the generation is decoration (`har-threat`'s rule about testing the control).

**SoA against AoS** pays when a pass touches one field of many across a large N — a cache line then carries sixteen ids instead of one struct — and costs you every operation that wants a whole element, plus the invariant that the parallel `Vec`s stay the same length. Make that invariant a type: one struct owning the `Vec`s, no public field, one `push` that writes all of them. Measure before splitting; `-Zprint-type-sizes` tells you whether AoS was even the problem.

**Bitsets** — `Vec<u64>` plus a length, or `[u64; N]` — beat a hash set for dense membership over a compact index space, which is exactly what a generational arena hands you. Bound the index before setting a bit.

**A per-frame scratch `Vec` reset with `clear()` is the arena you already have**: hoisted, capacity kept, no crate, and no escape analysis to get wrong. Reach for a bump allocator only when a profile shows many small *heterogeneous* allocations that all die together and a single typed `Vec` cannot hold them.

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

Reach for `&[u8]` rather than an unchecked conversion; `har-unsafe` owns any unsafe fast path. (`har-io` owns correctness when the bytes are user-visible text.)

## Iterators

- One terminal `collect()`. Intermediate ones allocate for nothing.
- `try_fold` / `try_for_each` when items are `Result`/`Option` or a condition stops the pipeline and the output is an accumulator — it short-circuits without materializing a temporary. Keep `collect` when you genuinely need the collection.
- `filter_map` over `filter().map()`. `extend` into an existing collection over collect-then-append.
- `iter().copied()` for small `Copy` types, and `chunks_exact` over `chunks` — both codegen better.
- **When the chunk size is a constant, `as_chunks::<N>()` beats `chunks_exact(N)`** (stable 1.88, with `as_chunks_mut` and `as_rchunks`). It yields `(&[[T; N]], &[T])` — real arrays plus the remainder — so indexing inside a chunk is checked at compile time rather than at run time, which deletes bounds checks *and* removes a panic site under a zero-panic budget (`har`). `clippy::chunks_exact_to_as_chunks` flags the constant-size case for you.
- `for x in &xs`, never `for i in 0..xs.len()`. Indexed access keeps the bounds check the iterator elides.
- Implement `size_hint` (or `ExactSizeIterator`) on custom iterators so `collect` and `extend` pre-allocate.

**When a hot loop must index** — zipped arrays, sliding windows — the safe ways to get the bounds check elided, in order: iterate or `zip` if the shape allows; otherwise narrow the slice **once** before the loop (`let xs = &xs[..n];`) or assert the range up front, so LLVM proves the rest. Read the generated code before believing it worked. `get_unchecked` is the last resort, and it converts a compiler-checked fact into an unsafe invariant you now own (`har-unsafe`).

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
const _: () = assert!(size_of::<FrameSlot>() == 64);
```

That is a const assert, so it needs no dependency and fails at compile time;
`static_assertions::assert_eq_size!` is the crate form of the same check.

## Hashing

`std::collections::HashMap` is the default and stays the default on anything a peer can key. Its docs: "By default, `HashMap` uses a hashing algorithm selected to provide resistance against HashDoS attacks. The algorithm is randomly seeded", and "The default hashing algorithm is currently SipHash 1-3, though this is subject to change at any point in the future." Strong, and slow on integer and short keys.

**Before reaching for a crate**, note that the hasher is a type parameter, so swapping it needs no dependency:

```rust
type FastMap<K, V> = HashMap<K, V, BuildHasherDefault<MyHasher>>;
```

A fast non-cryptographic `Hasher` for integer and short keys is tens of lines, lives in one module, and costs nothing in the dependency graph. Write that before admitting a crate for it — and measure against your real key distribution first, because the default is frequently not the bottleneck.

If a crate is admitted anyway (`har-supply` owns that decision), they are not interchangeable and none is a drop-in for a peer-keyed map:

| Map | Hasher | Use |
| --- | --- | --- |
| `std::collections::HashMap` | SipHash 1-3, randomly seeded | anything keyed by attacker-supplied data |
| `hashbrown::HashMap` | foldhash, with its `default-hasher` feature | foldhash's own docs call it "a fast, non-cryptographic, minimally DoS-resistant hashing algorithm" and tell you not to rely on it against someone studying the process to reverse its internal state |
| `rustc_hash::FxHashMap` | FxHasher — its docs claim only "a fast non-cryptographic hash", and say rustc is "not really worried about DOS attempts" | internal maps keyed by ids |
| `ahash::HashMap` | AHash, opt-in keyed | when its keying is actually configured |

Published rankings are measurements of one workload on one version and do not transfer; neither crate's docs state a speedup you can quote. Keep the default hasher on any map keyed by data a peer controls; `har-threat` owns that judgement.

## IO

`har-layout` owns stdout and stderr discipline, buffering, and explicit `flush` — a lock per call and a syscall per unbuffered read, which is why it shows up in a profile at all.

## Codegen cost

A generic function is compiled once per instantiation: build time, binary size, and instruction cache all pay. `cargo llvm-lines | head -20` names the offenders, `cargo bloat --release --crates` says what shipped. The fix — a thin generic shell delegating to one non-generic `inner` — is `har-layout`'s, under compile-time budget; here it matters because the duplicated bodies are what evict the icache.

## `#[inline]` and `#[cold]`

Default: no attribute. The compiler sees the body within a crate, and `lto` extends that across crates.

- Small non-generic public functions callers should inline across a crate boundary — `Deref`, `AsRef`, accessors: yes.
- Sprinkling it across a module costs build time and buys nothing.
- There is **no language rule against it on private or generic functions**, and the Performance Book explicitly recommends splitting a mixed hot/cold function into an `#[inline(always)]` hot half and an `#[inline(never)]` wrapper. So after a profile points at a call, `#[inline]` or `#[inline(always)]` anywhere is permitted — then re-measure, and check code size, because inlining loses to icache pressure as often as it wins.

The inverse is underused. When a profile shows a hot success path polluted by infrequent work — error formatting, a diagnostic, a recovery routine — extract it and mark the outlined function `#[cold]`, optionally with `#[inline(never)]`:

```rust
#[cold]
#[inline(never)]
fn report(span: Span, found: u8) -> Error { Error::Unexpected { span, found } }
```

For a branch rather than a whole function, `core::hint::cold_path()` (1.95) marks the arm itself without outlining anything — the finer tool when the cold work is three lines and extracting it would cost more clarity than it buys.

Not a blanket annotation for every `Err` — a simple check is cheaper left inline.

## Release-profile tuning

`har-supply` owns the profile as policy; these are the runtime trade-offs. **None of them has a promised number** — try it, measure it on the deployment target, and record the artifact and CPU compatibility you just committed to.

| Knob | Buys | Costs, and what it commits you to |
| --- | --- | --- |
| `lto = "thin"` | cross-crate inlining | modest link time |
| `lto = "fat"` | more of the same | much slower builds |
| `codegen-units = 1` | more inlining within a crate | loses build parallelism |
| `panic = "abort"` | smaller binary, no unwind tables | no `catch_unwind`, no unwind-based cleanup, no FFI unwind boundary |
| `-C target-cpu=native` | AVX and friends | binary runs only on that microarchitecture — controlled fleets only; otherwise a conservative baseline plus checked runtime dispatch |
| `mimalloc` / `jemalloc` | often large on allocation-heavy programs | a dependency, and the wrong first move — the **Allocation** section above removes allocations rather than making them cheaper, and a size-tuned binary pays for the allocator in bytes too |
| PGO | real, when the profile is representative | an instrument → run → merge → rebuild cycle, and a profile that must match production input |
| BOLT, after PGO | code layout and icache, on a final native binary | external LLVM BOLT tooling, Linux and deploy-controlled binaries only; `cargo-pgo` automates it and calls BOLT support experimental. Keep a non-BOLT fallback |

`overflow-checks = true` is real time in hot arithmetic, and `har-supply` sets it deliberately. Do not turn it off globally to win a benchmark: pick the explicit operation for the hot expression instead — `har` covers `checked_*` / `wrapping_*` / `saturating_*` — and keep the check everywhere else. Read it together with the panic strategy: under `panic = "abort"` a surprise overflow is a process abort, so the explicit operation is the control and `overflow-checks` is the backstop proving you did not miss one (`har`, `har-layout`).

PGO, concretely, since the table names it and not how to run it: `cargo pgo instrument build`, run the instrumented binary over representative input, `cargo pgo optimize build`. BOLT is a second pass on top — `cargo pgo bolt build --with-pgo`, then `cargo pgo bolt optimize --with-pgo` — and `cargo-pgo`'s own README calls BOLT support experimental, so keep a non-BOLT artifact.

## Profiles are per package, not per build

A profile applies to the whole graph; `[profile.<name>.package.<crate>]` applies to one. That is the knob for a cost you feel in the dev loop but not in the shipped binary — a syntax highlighter or a deserializer that takes hundreds of milliseconds unoptimized, inside the very frame that first needs it, while your own crates stay at `opt-level = 0` so a debugger is useful:

```toml
[profile.dev.package.some_highlighter]
opt-level = 3            # 300 ms of grammar compilation per first paint, unoptimized
```

Set it per named dependency, with the measured number in the comment. The reason it is worth knowing: the alternative people reach for is optimizing the whole dev profile, which costs build time on everything to fix one dependency.

Two precedence facts that bite:

- `[profile.X.package."*"]` **outranks** `[profile.X.build-override]`, so a `"*"` override also compiles every proc macro and build script at that level — minutes of CPU for code that never reaches the binary.
- A profile's own `opt-level` already covers dependencies, so `"*"` is usually redundant with it anyway.

`opt-level = "z"` against `3` is the same per-package decision. Size-tuning the whole binary and then restoring `3` for the one crate with a frame budget is normal, and should carry its measurement; a global `"z"` with no exception is a bet that nothing in the binary is latency-sensitive, which is worth stating rather than defaulting into.

For the dev loop only, the Cranelift backend (`-Zcodegen-backend=cranelift`) trades runtime speed for build speed. Nightly, dev-profile, and never the profile whose numbers a budget is measured against.

## SIMD

Autovectorization first: shape the loop so LLVM can do it — `as_chunks::<N>()` for a constant width, no early exits, no aliasing doubt — and then **confirm it happened**, because the whole approach is worthless unsupervised:

```
cargo rustc --profile profiling -- --emit asm        # then read target/*/deps/*.s
perf stat -e fp_arith_inst_retired.256b_packed_single ./app   # Linux: count the wide ops
```

That is most of the available win, with no unsafe and no dispatch. The **algebraic float methods** (`algebraic_add`, `_sub`, `_mul`, `_div`, `_rem`, stable 1.98) are the lever when a float loop will not vectorize because reassociation is not allowed: they permit it for that expression only, instead of turning on fast-math for the whole build.

Explicit SIMD on stable Rust means `std::arch` intrinsics with `#[target_feature]`, behind runtime detection, with a scalar fallback:

```rust
#[cfg(target_arch = "x86_64")]
fn sum(xs: &[f32]) -> f32 {
    if is_x86_feature_detected!("avx2") {
        // SAFETY: avx2 confirmed by the check above.
        unsafe { sum_avx2(xs) }
    } else { sum_scalar(xs) }
}
```

Calling a `#[target_feature]` function on a CPU without the feature is UB (`har-unsafe`, UB #7), so the detection is the whole safety argument. **`std::simd` / `core::simd` is still nightly**, behind `feature(portable_simd)` — its tracking issue is open and blocked on unresolved semantics — so it is allowed only under a pinned, reviewed nightly policy, never as a default for a stable build.

Which means that on stable, a crate is the only way to get portable explicit SIMD, and they are not equivalent. The axis that matters is **who picks the instruction set, and when**:

| Crate | Shape | Trade-off |
| --- | --- | --- |
| `memchr` | you write no SIMD; it detects at runtime on x86-64, aarch64 and wasm32 | solves byte search and substring search only — but solves them better than you will |
| `fearless_simd` | safe API, `dispatch!` selects the level at runtime, scalar fallback provided | the only option that is both safe and runtime-dispatched; the newest and least proven |
| `pulp` | `Arch::dispatch` / `WithSimd`, runtime dispatch, safe wrapper | an API and its lifetime rules to learn |
| `wide` | fixed-width types (`f32x8`, `u8x32`) | **no runtime dispatch** — the width is whatever compile-time target features allow, so a distributed binary gets the baseline and falls back to scalar |

So: `memchr` if the problem is search. Runtime dispatch (`pulp`, `fearless_simd`) for a binary you ship to machines you do not control. `wide` only with `-C target-cpu` and a controlled fleet, which is the commitment the release-profile table warns about. And in every case, measure against the autovectorized scalar version first — it is frequently already there.

## The 120fps checklist

8.3 ms per frame. Steady-state paint:

1. Zero allocations. Every per-frame buffer is a field on the renderer, `clear()`ed at frame start, never recreated. Growth amortizes to zero within a few frames.
2. No `format!`. Build the text where it is needed: `write!` into a `String` that is a field on the renderer and `clear()`ed per pass, or format a number straight into a stack buffer with no allocation at all — `{integer}::format_into` with a `core::fmt::NumBuffer`, and `fmt::from_fn` for a `Display` you do not want to declare a type for. Caching formatted text in the model and invalidating on change is the third option, and it buys speed by adding an invalidation path: if the format call is the cost, the cheaper fix is usually to format less, not to remember more.
3. No `collect()`. Iterate. If a collection is unavoidable, it is a hoisted one.
4. Every clone is a refcount bump. `Arc<str>` / `Arc<[T]>` for shared frame data. A `String` or `Vec` clone per element per frame is the classic killer.
5. Id-keyed lookups use a fast non-cryptographic hasher — your own `Hasher` behind `BuildHasherDefault`, or a crate if one is admitted — and the map stays allocated across frames. Better still, if the ids are dense, index a flat array instead of hashing at all (see **Flat storage**, above).
6. Hot structs stay small, and `assert_eq_size!` says so — layout regressions are otherwise invisible.
7. Anything unbounded — parse, IO, database — runs on a worker. The paint path reads a prepared snapshot.
8. No per-frame log line or span. Sample, or instrument the interaction instead: 120 formatted lines a second is cost and noise.

## Anti-patterns

| Anti-pattern | Fix |
| --- | --- |
| A benchmark with no `black_box` | `black_box(f(black_box(input)))`, and consume the result |
| `String::new()` / `Vec::new()` / `format!` inside a loop | hoist and `clear()`, or `write!` into a reused buffer |
| `.lines()` over a large file | `read_line` into one reused `String` |
| Hand-written `push` loop to append a slice | `extend_from_slice` |
| Chained `collect()`s | one terminal `collect`, or `extend`, or return `impl Iterator` |
| `collect::<Result<Vec<_>,_>>()` only to fold it | `try_fold` / `try_for_each` |
| `for i in 0..v.len() { v[i] }` | `for x in &v`; pre-slice or assert once when indexing is unavoidable |
| `ok_or(expensive())`, `unwrap_or(build())` | the `_else` variants |
| `v = other.clone()` | `v.clone_from(&other)` |
| `Arc<[T]>` for a payload that is never actually shared | `Box<[T]>` |
| `.clone()` added because the borrow checker complained | find the real conflict; `Arc`, `mem::take`, or restructure |
| Large generic body instantiated everywhere | thin generic wrapper delegating to a non-generic `inner` |
| `#[inline]` sprinkled across a module | delete; keep it where a profile put it, or turn on `lto` |
| Hot success path carrying rare error formatting inline | outline it, `#[cold]` |
| Default hasher on a hot internal id-keyed map | `BuildHasherDefault<MyHasher>`, or a flat array if the ids are dense; never the reverse on a peer-keyed map |
| `chunks_exact(N)` with `N` a constant | `as_chunks::<N>()` — `clippy::chunks_exact_to_as_chunks` catches it |
| `static_assertions::assert_eq_size!` for a size you can assert in `const` | `const _: () = assert!(size_of::<T>() == N);` |
| `target-cpu=native` in a distributed binary | baseline plus runtime feature detection |
| Optimizing against a debug build or a microbenchmark alone | realistic workload, release build, a profiler |
