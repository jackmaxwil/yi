---
name: har-concurrent
description: Rust shared-state concurrency — picking between channel, mutex, atomic and Once; memory ordering; Send/Sync; guard lifetime and spurious-wakeup traps; false sharing; building primitives. Load when threads share data or when writing anything with an Ordering argument.
---

# Shared state

## Pick the smallest tool

No shared state is the first answer. Move the data, or send it.

| Situation | Tool |
| --- | --- |
| Handed over once, never shared | move it into the thread |
| Producer-consumer stream | channel (`mpsc`), never `Mutex<Vec<_>>` + polling |
| One-time init | `OnceLock` / `LazyLock` |
| Compound invariant over >1 field | `Mutex` |
| Single word, no invariant with anything else | atomic |
| Reads dominate *and* the critical section is long | `RwLock` |
| Reads dominate, section short | `Mutex` — the reader path is a CAS either way |
| Read-mostly whole structure, hot | swap an `Arc<T>` wholesale |

A lock is the wrong tool when it is held across `.await` or a syscall, contended by a render thread, guards one word, guards data only one thread mutates, or guards an allocation rather than an invariant.

## Thread lifetime

`thread::spawn` requires `'static`. Four ways out, in preference order: `thread::scope` (borrow locals directly, joins all children at scope exit, propagates their panics) → `Arc` → `static` → `Box::leak`. `Box::leak` is legitimate once per process, never in a loop.

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

## Mutex and RwLock

`lock()` returns `Result` only because of poisoning: a thread panicked mid-mutation and the data may violate its own invariant. Decide, per lock: propagate a `PoisonError` variant; `into_inner()` when the invariant provably survives; or `parking_lot` when poisoning is never wanted. `.lock().unwrap()` fans one thread's panic out to every thread — see `har` for the general rule.

**Guard lifetime.** A plain `if` drops temporaries before the body. `if let`, `match`, and `while let` hold them until the end of the whole statement, so the second form below stays locked through `process`:

```rust
let item = list.lock()?.pop();
if let Some(item) = item { process(item); }

if let Some(item) = list.lock()?.pop() { process(item); }
```

Minimize the locked region, not the lock count. `drop(guard)` explicitly before slow work. One lock, or a strict global lock order. `std` has no deadlock detection. Never share a lock between a latency-critical thread and a background worker: `std` has no priority-inheritance mutex, so that is a priority inversion and a dropped frame. Hand data over instead.

## Condvar and park

`Condvar::wait(guard)` atomically unlocks, sleeps, relocks. Always in a `while` over the predicate, never `if` — wakeups are spurious. Pair each `Condvar` with exactly one `Mutex`; mixing may panic.

`notify_one` when any single waiter can progress; `notify_all` only when the state change is relevant to all of them, else N wake and N-1 sleep again. `thread::park` / `Thread::unpark`: unpark requests do not stack, an unpark *before* the park is not lost, and `park` returns spuriously. Same `while`-loop rule.

## One-time init

`OnceLock` / `LazyLock` before any hand-rolled CAS. `Once`, `Barrier`, and `mpsc` are in `std::sync` too.

# Atomics

## The operations

`load`, `store`, `swap`, `fetch_add/sub/or/and/xor/max/min`, `compare_exchange`, `compare_exchange_weak`, `fetch_update`, plus `get_mut` and `into_inner`, which take no ordering because `&mut`/ownership proves exclusivity. `fetch_*` returns the **old** value. `fetch_add`/`fetch_sub` wrap silently — `overflow-checks` does not apply to atomics, so a counter that can reach the max needs its own guard (`har` owns the scalar arithmetic rules):

```rust
NEXT_ID.fetch_update(Relaxed, Relaxed, |n| n.checked_add(1)).map_err(|_| Error::IdSpace)?
```

## The CAS loop

`compare_exchange_weak` inside a loop, `compare_exchange` outside one. Failure ordering may be weaker than success and is `Relaxed` whenever the failure branch touches nothing shared.

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

`SeqCst` adds one global total order over all `SeqCst` operations and nothing else. Treat it in a diff as a review flag, not a safety margin.

| Operation | Legal orderings |
| --- | --- |
| `load` | `Relaxed`, `Acquire`, `SeqCst` |
| `store` | `Relaxed`, `Release`, `SeqCst` |
| RMW, CAS success | all |
| CAS failure | the `load` orderings only |

Illegal combinations panic at **runtime**, not compile time. There is no acquire-store and no release-load; reaching for one means the design is wrong.

## Fences

`fence(Acquire)` / `fence(Release)` detach the ordering from any one variable: use them when the edge must cover several atomics, or only on a rare branch — `if last { fence(Acquire); }` beats paying `AcqRel` every iteration. `compiler_fence` orders the compiler only, never the CPU; signal handlers only, never a substitute.

# Cost

A cache line is ~64 bytes and coherence is MESI: any write claims the line exclusively and invalidates every other core's copy. **False sharing:** three `AtomicU64` in one array cost seconds where padded ones cost ~300 ms — a ~10x cliff from adjacency alone. Pad hot per-thread cells:

```rust
#[repr(align(64))]
struct Counter(AtomicU64);
```

A **failed** `compare_exchange` still claims the line exclusively. Spin on `load(Relaxed)` and only CAS when the load looks promising, with `std::hint::spin_loop()` in the body.

**x86-64 hides ordering bugs.** There, relaxed/acquire/release loads and stores compile to the same instruction as non-atomic ones; only a `SeqCst` store costs extra, and `compare_exchange_weak` is identical to the strong form. On aarch64 `Relaxed` is genuinely cheaper (`ldr`/`str` vs `ldar`/`stlr`) and pre-v8.1 RMWs are LL/SC loops, so `_weak` pays. A relaxed-where-acquire-was-needed bug can be perfect on x86 and wrong on an M1. CI runs aarch64, or `loom`.

# Building primitives

Don't. `std::sync` first, then `parking_lot` / `crossbeam` / `atomic-wait` (`har-supply` owns the add-or-not decision). A hand-rolled lock, channel, or `Arc` needs `UnsafeCell` plus `unsafe impl Send/Sync`, which cannot live under the workspace `forbid(unsafe_code)` posture — isolate it in one crate with `deny`, never weaken the workspace setting. If you must: encode lock state as an enum, not magic `u32`s; three states (unlocked / locked / locked-with-waiters) so the uncontended path makes no syscall; spin ~100 times before sleeping on `wait`/`wake_one`; `#[cold]` the contended path. Guard checklist: lifetime parameter, `UnsafeCell` payload, `Deref` (`DerefMut` only when exclusive), `Drop` to unlock, explicit `unsafe impl Send/Sync ... where T: ...`. `har-unsafe` owns the invariants.

`Arc`'s orderings are the canonical worked example:

| Op | Ordering | Why |
| --- | --- | --- |
| clone, `fetch_add` | `Relaxed` | nothing is being published |
| drop, `fetch_sub` | `Release` | publish your writes to whoever drops last |
| final drop | `fence(Acquire)`, then free | acquire every prior release before touching the data |
| `get_mut`, count == 1 | `load(Relaxed)` + `fence(Acquire)` | same edge; `&mut self` proves uniqueness |

Refcount overflow is memory-unsafe, so it `abort()`s above `usize::MAX / 2` rather than returning an error — that many live threads cannot exist.

# Async

The executor is a thread pool with no preemption: anything that blocks a worker — a contended `Mutex`, `join`, `park`, file IO, a long CPU loop — stalls every other task on that worker. Two rules belong here because they are shared-state rules: `std::sync::Mutex` is the right default in async code (lock, mutate, drop the guard, never await while holding it — the guard is `!Send`, so the compiler says so), and a lock shared between a render thread and an async worker is the priority inversion above.

Everything else about async — runtime shape, task lifecycle, cancellation, shutdown, channels, subprocess stdio, framing, timeouts — is `har-async`.

# Traps

- `if let` / `match` / `while let` holds the guard for the whole statement.
- `if` instead of `while` around `Condvar::wait` or `thread::park`.
- `notify_all` where `notify_one` was meant: thundering herd.
- `fetch_add` wraps silently; debug builds do not catch it.
- ABA — a CAS succeeds across A→B→A. Matters for pointers and reused indices; add a generation counter.
- `mem::forget` on a guard or `Arc` leaks the lock. Leaks are safe, so no API may depend on `Drop` running.
- `compiler_fence` mistaken for `fence`.
- `SeqCst` applied as a fix.
- `.lock().unwrap()` on a poisoned mutex.
- pthread mutexes/condvars are not movable, and destroying a locked one is UB — never expose one by value across FFI.

# Verification

`loom` is `har-verify` rung 7, and this is how you use it. It enumerates interleavings and weak-memory outcomes exhaustively, so it finds the ordering bug x86 hid:

```toml
[target.'cfg(loom)'.dependencies]
loom = "0.7"
```

```
RUSTFLAGS="--cfg loom" cargo test --release --test loom
```

Swap `std::sync::atomic` and `std::thread` for `loom::sync::atomic` and `loom::thread` under `cfg(loom)`, wrap the scenario in `loom::model(|| ...)`, and keep it to 2-3 threads and a handful of operations — cost is exponential. Then `MIRIFLAGS="-Zmiri-many-seeds=0..64" cargo +nightly miri test` to re-run under many randomized schedules; it catches data races `loom` is not modelling. Neither replaces an aarch64 job in CI.
