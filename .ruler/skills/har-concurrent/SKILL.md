---
name: har-concurrent
description: Rust shared-state concurrency — picking between channel, mutex, condvar, atomic and Once; memory ordering; Send/Sync; guard lifetime and spurious-wakeup traps; false sharing and cache padding; atomic pointers, reclamation and ABA; building primitives. Load when threads share data or when writing anything with an Ordering argument.
---

# Shared state

## Pick the smallest tool

No shared state is the first answer. Move the data, or send it.

| Situation | Tool |
| --- | --- |
| Handed over once, never shared | move it into the thread |
| Producer-consumer stream, ownership transferred | channel — `std::sync::mpsc::sync_channel(n)` when producer backpressure is part of correctness, `channel()` only when unbounded growth is acceptable |
| A shared predicate several threads wait on | `Mutex` + `Condvar::wait_while` — blocks without polling and preserves an arbitrary invariant |
| One-time init | the `Once` family, below |
| Compound invariant over >1 field | `Mutex` |
| Single word, no invariant with anything else | atomic |
| Reads dominate, contention measured, writer latency acceptable | `RwLock` |
| Anything else read-heavy | `Mutex` — the reader path is a CAS either way |
| Read-mostly whole structure, hot | swap an `Arc<T>` wholesale — then see **reclamation**, below |

A channel is not categorically better than a lock; it is better when the model is *ownership transfer*. A shared invariant that several threads wait on is a `Condvar`'s job, not a channel's. What is always wrong is polling a `Mutex<Vec<_>>` when a blocking primitive fits.

`RwLock` is not chosen by critical-section length. Its fairness policy comes from the OS and can favour readers or writers, so starvation behaviour is not portable. Choose it only on a measured read-concurrency win plus an acceptable writer-latency story on every supported platform; `Mutex` otherwise.

A lock is the wrong tool when it is held across `.await` or a syscall, contended by a render thread, guards one word, guards data only one thread mutates, or guards an allocation rather than an invariant.

## Thread lifetime

`thread::spawn` requires `'static`. The options are not a ladder of last resorts; they mean different things:

| Need | Use |
| --- | --- |
| Children borrow locals and **all finish before the scope returns** | `thread::scope` — joins every child at scope exit and propagates their panics |
| Genuine shared ownership across independently-lived threads | `Arc` |
| A value whose process-lifetime semantics are intentional | `static` |
| One deliberate, documented leak at startup | `Box::leak` — legitimate once per process, never in a loop |

`thread::scope` is not an ownership substitute for work that must outlive its caller, and `static`/`Box::leak` are not "the next rung" — reach for them only when process lifetime is what you actually mean and teardown is genuinely unnecessary.

```rust
let numbers = vec![1, 2, 3];
thread::scope(|s| {
    s.spawn(|| report(numbers.len()));
    s.spawn(|| report(numbers.iter().sum::<i32>()));
});
```

Returning from `main` kills every other thread. A detached thread's remaining work is not "later", it is "never". `thread::spawn` and `JoinHandle::join` are themselves happens-before edges: data handed over that way needs no ordering.

## Send and Sync

`Send` = the value may move to another thread. `Sync` = `&T` is `Send`. Both are auto traits, derived structurally — one `Rc` field makes the whole struct thread-local, and the error names the field, not the type you were thinking about.

| Symptom | Cause | Fix |
| --- | --- | --- |
| `Rc<..> cannot be sent between threads` | `Rc` in a spawned closure | `Arc`, or move the whole graph |
| `RefCell<..> cannot be shared` | `RefCell` behind `Arc` | `Mutex`/`RwLock`, or keep it thread-local |
| `MutexGuard is not Send` | guard alive across an `.await` | scope the guard, drop before the await |
| `*mut T is not Send` | FFI handle in a struct | prove it, then `unsafe impl`, citing the C library's docs |
| compiles but races | `unsafe impl Send` added to silence the error | delete it |

`unsafe impl Send for X {}` is a proof obligation: the compiler's complaint is the finding, not the obstacle. Opt *out* at zero cost with a marker field: `PhantomData<Cell<()>>` drops `Sync`, `PhantomData<*const ()>` drops `Send` (pinning a receiver to the thread that will be unparked). This is `PhantomData` as a thread-safety marker, unrelated to the call-order typestate in `har`.

`std::sync::Exclusive<T>` is sometimes proposed here. It is **nightly-only** (`exclusive_wrapper`), and it is not a lock: it is unconditionally `Sync` because it hands out only `&mut`, relying on the borrow checker rather than on runtime mutual exclusion. Useful for wrapping a non-`Sync` future in an interface whose access is statically exclusive; never a substitute for `Mutex` where threads actually compete.

## Mutex and RwLock

`lock()` returns `Result` only because of poisoning: a thread panicked mid-mutation and the data may violate its own invariant. Decide, per lock: propagate a `PoisonError` variant; `into_inner()` when the invariant provably survives; or `parking_lot` when poisoning is never wanted. `.lock().unwrap()` fans one thread's panic out to every thread — see `har` for the general rule.

**Guard lifetime.** A plain `if` drops temporaries before the body. A `match` scrutinee's temporaries live to the end of the whole statement, so the second form below stays locked through `process`:

```rust
let item = list.lock()?.pop();
if let Some(item) = item { process(item); }

if let Some(item) = list.lock()?.pop() { process(item); }
```

Both forms still differ, but **the exact drop point is edition-dependent and per-construct**, so do not carry a single mnemonic across all three. In edition 2024 an `if let` scrutinee temporary is dropped **before** the `else` block, where in earlier editions it lived to the end of the statement; `match` keeps the older statement-wide behaviour; `while let`'s scrutinee temporary is scoped to the condition and body. Read the Reference's destructors chapter for the construct you are using — and where the unlock point matters for correctness, bind the guard to a `let` or wrap it in an inner block instead of relying on any of this.

`RwLockWriteGuard::downgrade` (1.92) turns a write guard into a read guard **without releasing the lock**, so a read-modify-then-serve path does not have to drop and re-acquire — which is where a writer loses the lock to a reader it was about to serve.

Minimize the locked region, not the lock count. `drop(guard)` explicitly before slow work. One lock, or a strict global lock order. `std` has no deadlock detection. Never share a lock between a latency-critical thread and a background worker: `std` has no priority-inheritance mutex, so that is a priority inversion and a dropped frame. Hand data over instead. Priority inheritance, where a real-time deployment needs it, is an OS and lock-implementation policy — Rust's atomics supply none of it.

## Condvar and park

`Condvar::wait(guard)` atomically unlocks, sleeps, relocks. Always in a `while` over the predicate — or use `wait_while`, which is that loop — never `if`, because wakeups are spurious. Pair each `Condvar` with exactly one `Mutex`; mixing may panic.

`notify_one` when any single waiter can progress; `notify_all` only when the state change is relevant to all of them, else N wake and N-1 sleep again. `thread::park` / `Thread::unpark`: unpark requests do not stack, an unpark *before* the park is not lost, and `park` returns spuriously. Same `while`-loop rule.

## One-time init

| Need | Single-threaded | Thread-safe |
| --- | --- | --- |
| Set once, read many, initialized explicitly | `OnceCell` | `OnceLock` |
| Initialized on first access by a closure fixed at declaration | `LazyCell` | `LazyLock` |
| One-time **side effect**, no stored value | — | `Once` |

`Once` is not obsolete: it is the primitive for run-this-exactly-once, and `call_once_force` is the only way to recover after an initializing panic poisoned it. Panic behaviour differs and matters: a panic inside `LazyLock`'s initializer poisons it permanently, while a panic inside `OnceCell::get_or_init` leaves the cell **uninitialized** so the next caller retries. `OnceLock` also has reentrancy caveats — an initializer that re-enters the same lock deadlocks or panics depending on the method. `Barrier` and `mpsc` are in `std::sync` too.

# Atomics

## Availability

`std` guarantees that an atomic type which **exists** on the target is lock-free — not wait-free, and not present everywhere, with no promised lock-based fallback. So anything reaching for `AtomicU64` on a small target, for `AtomicU128`, or for pointer-width atomics gates on capability and ships the alternative. `har-target` owns `target_has_atomic`, its accepted widths, and the shipped fallback.

## The operations

`load`, `store`, `swap`, `fetch_add/sub/or/and/xor/max/min`, `compare_exchange`, `compare_exchange_weak`, `fetch_update`, plus `get_mut` and `into_inner`, which take no ordering because `&mut`/ownership proves exclusivity. `fetch_*` returns the **old** value. `fetch_add`/`fetch_sub` wrap silently — `overflow-checks` does not apply to atomics, so a counter that can reach the max needs its own guard (`har` owns the scalar arithmetic rules):

```rust
NEXT_ID.fetch_update(Relaxed, Relaxed, |n| n.checked_add(1)).map_err(|_| Error::IdSpace)?
```

## The CAS loop

**Reach for `update` or `fetch_update` first.** `update` / `try_update` exist on every atomic type since 1.95 (`AtomicBool`, `AtomicPtr` and the integers), and `fetch_update` predates them: they take the two orderings and a closure and run the retry loop for you, which is the loop everyone writes slightly wrong.

```rust
NEXT_ID.fetch_update(Release, Relaxed, |n| n.checked_add(1)).map_err(|_| Error::IdSpace)?
```

Write the loop by hand only when the retry needs to do something the closure cannot — bail out, await, or touch another variable. Then: `compare_exchange_weak` inside a loop, `compare_exchange` outside one. Failure ordering may be weaker than success and is `Relaxed` whenever the failure branch touches nothing shared.

```rust
let mut current = a.load(Relaxed);
loop {
    match a.compare_exchange_weak(current, compute(current), Release, Relaxed) {
        Ok(_) => break,
        Err(v) => current = v,
    }
}
```

Racing to initialize is fine when the value is idempotent. When a single winner matters, `compare_exchange` and adopt the loser's value from the `Err`.

A **failed** CAS is documented as *not a write* in Rust's model — reason about it as a load with the failure ordering. Whether the target's implementation nonetheless claims the cache line exclusively is a microarchitecture question, not a portable guarantee; where failed-CAS contention shows up in a profile, the mitigation is to spin on `load(Relaxed)` and CAS only when the load looks promising.

## Memory ordering

`Relaxed` guarantees exactly one thing: a total modification order **per atomic variable**, agreed on by all threads; it relates nothing to any other variable, atomic or not. A `Release` store paired with an `Acquire` load *of that same value* creates happens-before: everything sequenced before the store is visible after the load. That is the only mechanism that publishes non-atomic data. On a read-modify-write, `Acquire` covers the load half, `Release` the store half, `AcqRel` both. A released value stays released through a chain of relaxed RMWs on that atomic; a plain non-RMW `store` breaks the chain.

| Intent | Ordering |
| --- | --- |
| Counter nobody synchronizes on | `Relaxed` |
| Publish data written before the flag | `Release` store |
| Consume data written before that flag | `Acquire` load |
| Both — lock acquire on a CAS, refcount handoff | `AcqRel` |
| Needed only on the last or rare iteration | `Relaxed` op + conditional `fence` |
| Two threads each store, then read the other's flag | `SeqCst`, the only real case |
| "Not sure" | not `SeqCst` — the algorithm is wrong, not the ordering |

`SeqCst` adds one global total order over all `SeqCst` operations — **including `SeqCst` fences** — and nothing else; weaker operations do not join that order except through the fence and happens-before rules. A `SeqCst` operation also **keeps** its acquire/release effect, so it can synchronize with a weaker acquire or release on the same variable. Treat it in a diff as a review flag, not a safety margin.

| Operation | Legal orderings |
| --- | --- |
| `load` | `Relaxed`, `Acquire`, `SeqCst` |
| `store` | `Relaxed`, `Release`, `SeqCst` |
| RMW, CAS success | all |
| CAS failure | the `load` orderings only |

Illegal combinations panic at **runtime**, not compile time. There is no acquire-store and no release-load; reaching for one means the design is wrong.

## Fences

`fence(Acquire)` / `fence(Release)` detach the ordering from any one variable: use them when the edge must cover several atomics, or only on a rare branch — `if last { fence(Acquire); }` beats paying `AcqRel` every iteration. `compiler_fence` orders the compiler only, never the CPU; signal handlers only, never a substitute.

## Atomic pointers

`AtomicPtr` makes the *replacement* atomic. It makes nothing else safe, and every one of the following is a separate proof obligation before a dereference:

- **Publication ordering** — `Release` on the store, `Acquire` on the load, or the pointee's initialization is not visible.
- **Lifetime and reclamation** — see below. An `Acquire` load tells you nothing about whether the old allocation is still alive.
- **ABA** — a CAS succeeds across A→B→A. Matters for pointers and for reused indices; add a generation counter.
- **Alignment** — `AtomicPtr::from_ptr` requires the `AtomicPtr`'s own alignment, and availability is gated on `target_has_atomic = "ptr"`.
- **Provenance** — use `AtomicPtr`, never an integer atomic holding an address. `AtomicPtr` has had the arithmetic and bit operations since 1.91 — `fetch_ptr_add` / `fetch_ptr_sub` in units of `T`, `fetch_byte_add` / `fetch_byte_sub` in bytes, and `fetch_or` / `fetch_and` / `fetch_xor` for tag bits — so a tagged pointer no longer needs a load-modify-CAS loop and no longer needs to leave the pointer type. Outside an atomic, the `map_addr` family is the provenance-preserving form; masking through `usize` loses it either way (`har-unsafe` owns the provenance model).

**Reclamation is mandatory, not optional.** Swapping an `Arc<T>` or a raw pointer wholesale publishes the new value but does not tell you when the last reader of the old one is finished. Pick a mechanism and write it down: reference counting (`arc-swap`), hazard pointers, epoch-based reclamation (`crossbeam-epoch`), or quiescent-state tracking. EBR has its own contract — pin before loading a protected pointer, keep guards short, never carry a guard-derived reference across a `repin` or an unpinned gap, and expect deferred destruction to stall for as long as any participant stays pinned.

**Do not hand-roll a seqlock over ordinary fields.** A sequence counter detects an overlapping write, but the concurrent non-atomic read and write of the payload is still a data race and therefore UB in Rust — retrying after the counter changed does not undo it. A sound Rust seqlock needs atomic or otherwise race-free payload access, the right fences, retry handling, and normally a `Copy` payload. Use an audited implementation for that narrow read-mostly case, and a normal lock when readers need references or the payload owns pointers.

# Cost

Coherence is MESI: any write claims a cache line exclusively and invalidates every other core's copy, so two hot values sharing one line serialize for no reason. That is **false sharing**, and it is worth real multiples on a contended counter array.

Do not hardcode a line size. There is no portable one, and the effective granule is not always the nominal line: `crossbeam_utils::CachePadded` deliberately assumes **128 bytes on x86-64 and aarch64** — 128 on x86-64 to cover Intel's adjacent-line prefetch despite nominal 64-byte lines — 256 on s390x, 32 on arm/mips/sparc/hexagon, 16 on m68k, and 64 elsewhere, and its own docs call these reasonable guesses rather than hardware facts. Apple documents `sysctl hw.cachelinesize` as the way to ask.

So: align hot per-thread cells deliberately, and say which target the number came from.

```rust
// 128 rather than 64: on x86-64 the adjacent-line prefetcher pulls the
// neighbour, so a nominal 64-byte line is not the effective granule.
#[repr(align(128))]
struct Counter(AtomicU64);

struct Counters(Box<[Counter]>);
```

That needs no dependency and is honest about being one target's number. `crossbeam_utils::CachePadded` is the portable form of the same idea, carrying a per-architecture table its own docs call reasonable guesses rather than hardware facts — worth a dependency when you ship to architectures you have not measured, and not otherwise (`har-supply`).

**x86-64 hides ordering bugs.** For ordinary loads and stores, relaxed/acquire/release compile to the same instructions as non-atomic ones there; only a `SeqCst` store or fence costs extra. Read-modify-writes are a different matter on every target — they lower to `lock`-prefixed instructions or an LL/SC loop, never to a plain load-operate-store. On aarch64 `Relaxed` is genuinely cheaper (`ldr`/`str` vs `ldar`/`stlr`) and pre-v8.1 RMWs are LL/SC loops, so `compare_exchange_weak` pays where on x86-64 it lowers the same as the strong form. A relaxed-where-acquire-was-needed bug can be perfect on x86 and wrong on an M1. CI runs aarch64, or `loom`. Every one of these statements is a codegen observation for a particular compiler and target — check the assembly, do not treat it as a source-level contract.

**Spinning has a bounded form only.** `std::hint::spin_loop()` is a processor hint, not a scheduler operation, and `thread::yield_now` still busy-waits when nothing else is runnable. So: spin a bounded number of times on an expected-short wait, with `spin_loop()` in the body, then park or block. Unbounded spinning is how a low-priority holder starves a high-priority waiter.

**Sharding** removes most synchronization from a hot path: give each worker its own state and message across shards. It is not free — fixed affinity risks severe imbalance when work is uneven, which is the trade work-stealing schedulers make in the other direction, buying utilization with much more synchronization. Choose fixed affinity only with a load-balance plan.

# Building primitives

Don't. `std::sync` first, then `parking_lot` / `crossbeam` / `atomic-wait` (`har-supply` owns the add-or-not decision). A hand-rolled lock, channel, or `Arc` needs `UnsafeCell` plus `unsafe impl Send/Sync`, which cannot live under a workspace `forbid(unsafe_code)` posture — isolate it in one crate with `deny`, never weaken the workspace setting. If you must: encode lock state as an enum, not magic `u32`s; three states (unlocked / locked / locked-with-waiters) so the uncontended path makes no syscall; spin a bounded number of times before sleeping on `wait`/`wake_one`; `#[cold]` the contended path. Guard checklist: lifetime parameter, `UnsafeCell` payload, `Deref` (`DerefMut` only when exclusive), `Drop` to unlock, explicit `unsafe impl Send/Sync ... where T: ...`. `har-unsafe` owns the invariants.

`Arc`'s orderings are the canonical worked example:

| Op | Ordering | Why |
| --- | --- | --- |
| clone, `fetch_add` | `Relaxed` | nothing is being published |
| drop, `fetch_sub` | `Release` | publish your writes to whoever drops last |
| final drop | `fence(Acquire)`, then free | acquire every prior release before touching the data |
| `get_mut`, count == 1 | `load(Relaxed)` + `fence(Acquire)` | same edge; `&mut self` proves uniqueness |

Refcount overflow is memory-unsafe, so it `abort()`s above `usize::MAX / 2` rather than returning an error — that many live threads cannot exist.

# Async

The executor is a thread pool with no preemption: anything that blocks a worker — a contended `Mutex`, `join`, `park`, file IO, a long CPU loop — stalls every other task on that worker. Two rules belong here because they are shared-state rules: `std::sync::Mutex` is the right default in async code (lock, mutate, drop the guard, never await while holding it — the guard is `!Send`, so the compiler says so on a spawned task), and a lock shared between a render thread and an async worker is the priority inversion above.

Everything else about async — runtime shape, task lifecycle, cancellation, shutdown, channels, subprocess stdio, framing, timeouts — is `har-async`.

# Traps

- Guard drop points differ by construct **and by edition**. Bind the guard explicitly where the unlock point matters.
- `if` instead of `while` around `Condvar::wait` or `thread::park`; `wait` where `wait_while` says it better.
- `notify_all` where `notify_one` was meant: thundering herd.
- `fetch_add` wraps silently; debug builds do not catch it.
- ABA — a CAS succeeds across A→B→A. Add a generation counter.
- An `AtomicPtr` swap with no reclamation scheme: the load is ordered, the old allocation is freed under a live reader.
- A hand-rolled seqlock over non-atomic fields — still a data race, still UB.
- `mem::forget` on a guard or `Arc` leaks the lock. Leaks are safe, so no API may depend on `Drop` running.
- `compiler_fence` mistaken for `fence`.
- `SeqCst` applied as a fix, or read as isolating an operation from acquire/release synchronization.
- `.lock().unwrap()` on a poisoned mutex. `LazyLock` whose initializer panicked, which stays poisoned forever.
- `AtomicU128` or `AtomicPtr` used without a `target_has_atomic` gate.
- Unbounded spinning offered as a substitute for blocking.
- pthread mutexes/condvars are not movable, and destroying a locked one is UB — never expose one by value across FFI.

# Verification

`har-verify` rung 7 owns the setup, the cost model and what a green run does and does not prove. Three facts belong here because they are about ordering rather than about tooling:

- Loom finds exactly the bug x86-64 hides, because it explores the orderings the hardware happens not to exhibit — which is why "it passes on my machine" is not evidence for an `Ordering` argument.
- Loom treats `SeqCst` as `AcqRel` and skips some load-buffering outcomes, so it cannot vindicate a `SeqCst` you could not justify in the first place.
- Neither loom nor miri replaces **an aarch64 job in CI**. That is the cheapest of the three and the one most often missing.
