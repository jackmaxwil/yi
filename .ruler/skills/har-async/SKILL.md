---
name: har-async
description: Async Rust — executor shape and what blocking costs, task lifecycle and two-phase shutdown, cancel safety, sync primitives under await, Pin, actors, subprocess stdio and pipe deadlocks, framing codecs, streams, timeouts, async tests. Load when spawning a task or a child process, when wiring a stdio or socket protocol, or when a future can be dropped mid-flight.
---

# Async

An executor polls futures on a fixed set of threads with no preemption. Rules below are stated for any executor; rules naming a tokio-only API say so, and the subprocess, pipe, and EOF rules hold unchanged for blocking code on a plain `std::thread`. Where an application runs a UI toolkit's own executor (GPUI, winit, an embedded loop) instead of tokio, the tokio-named APIs have local equivalents and the shape of every rule survives the substitution.

**A future does nothing until something polls it.** Calling an `async fn` builds a state machine and returns; no work has started. `join!` and `select!` multiplex their branches **on the current task and thread** — that is concurrency, not parallelism, and one branch that blocks stops every sibling. Independent execution means `spawn`, and then the handle is the only thing that carries the result, the panic, and the lifetime.

## Blocking

"Blocking the thread" means preventing the runtime from swapping the current task. Blocking one worker stalls every task queued behind it; blocking a single-threaded executor — a UI toolkit's foreground executor, or `new_current_thread` — stalls the process, which at 120fps is a dropped frame, not a slowdown.

| Work | Tool |
| --- | --- |
| Blocking syscall, filesystem, blocking DB driver | `tokio::task::spawn_blocking`, or the toolkit's background-spawn equivalent |
| CPU-bound parallel compute | `rayon` — its pool is sized to cores; a blocking pool caps near 500 threads, which is oversubscription, not parallelism |
| A blocking loop that never returns (a sync `recv()` reader, a device thread) | dedicated `std::thread` + channel — a pool thread would be consumed permanently |
| Short CPU burst inside an async task, multi-thread runtime only | `tokio::task::block_in_place` — panics on a current-thread runtime, and suspends this task's `join!` siblings |
| Long CPU loop whose code you own | chunk it and `tokio::task::consume_budget().await` between chunks |

**There is no microsecond budget between awaits.** A common rule of thumb says 10–100 µs, and it is a useful smell test, but it is not a scheduling contract: awaiting an already-ready future need not yield at all, and `yield_now()` is documented to give no guarantee that any other task or the IO driver runs next. Responsiveness is a measured latency property of the whole system. What you can actually control is the amount of work between *cooperation points*, so chunk long loops and cooperate explicitly.

Tokio gives each task a cooperative budget, currently initialized to 128 operations per "tick"; past it, tokio's own resources return `Pending` until the task yields. Treat that number as a version-pinned implementation detail — tokio's source says it was chosen somewhat arbitrarily — never as a constant to test against or batch against. It covers cooperating tokio operations only, and cannot detect a genuinely blocking call.

A `spawn_blocking` task cannot be aborted once it has started, so bound it by its own means — a stop flag it polls, a channel it selects on, a chunked loop. Never nest runtimes: `Runtime::block_on` and `Handle::block_on` panic inside an async context. `futures::executor::block_on` does not panic; it runs the future to completion on the calling thread, so a self-contained future returns fine and one that needs the surrounding runtime's driver hangs the worker it is standing on. Either way it is the wrong async-to-sync bridge on a runtime thread: make the caller async, or establish the synchronous boundary outside the runtime.

## Runtime shape (tokio-specific)

`#[tokio::main]` **by default** expands to roughly `Builder::new_multi_thread().enable_all().build().unwrap().block_on(..)`; the macro also accepts `flavor = "current_thread"` and `worker_threads`, so read the attribute before assuming parallelism. That `unwrap` is the macro's; a hand-built `Builder` returns `io::Result<Runtime>` and lets the error reach the UI (`har` owns the rule).

- Drivers are **off** on a hand-built runtime. Without `enable_all()` / `enable_time()` a timer or socket **fails at runtime, not at compile time**.
- `new_multi_thread`: workers default to core count, `max_blocking_threads` to 512, each worker's local queue holds 256 tasks, and the LIFO slot holding the last-woken task is not stealable, so a long poll parks whatever sits behind it. `new_current_thread` runs only inside `block_on`; once it returns, its spawned tasks freeze.
- **`tokio::spawn` demands `Send + 'static` on a current-thread runtime too.** The single thread does not relax the API bound. `!Send` state (a UI handle, an `Rc`) may exist only *between* awaits inside a spawned future; state that must live across one needs `LocalSet` + `spawn_local` or `LocalRuntime`, and locally spawned tasks do not run until the `LocalSet` is polled.
- To bridge sync → async, put the runtime on its own `std::thread` driven by a bounded channel; `Handle::block_on` submits to an existing runtime, it does not drive one.
- **On a single-threaded runtime none of the multi-thread tuning above applies, and `block_in_place` panics unconditionally.** That is the case for a UI toolkit's executor, for `new_current_thread`, and for any build that omits tokio's `rt-multi-thread` feature. A call that can only ever panic is not a trade-off to weigh at review time: put it in the project's `disallowed-methods` list (`har-supply`) and let the lint carry it. Note also that without the `fs` feature there is no `tokio::fs`, which makes the `std::fs`-in-async trap below a non-issue by construction rather than by discipline.

## Task lifecycle

- `spawn` demands `Future + Send + 'static`: the task owns everything, borrowing a local is impossible, and there is no async `thread::scope` — `JoinSet` and `TaskTracker` are the substitutes.
- **Dropping a `JoinHandle` detaches.** The task keeps running, its panic is invisible, and shutdown does not wait for it. Hold the handle, or put it in a set.
- `abort()` is a request, and it is **not a barrier**. It returns before cancellation has happened; the cancellation lands at the task's next await point, synchronous code between awaits runs to completion, and a task that never yields again may finish normally instead. Anything that frees a resource the task still holds, or reports termination, must `await` the `JoinHandle` afterwards and handle both a normal result and `JoinError::is_cancelled()`. A started `spawn_blocking` job cannot be aborted at all.
- Awaiting a `JoinHandle` gives `Result<T, JoinError>`; `is_panic()` / `is_cancelled()` separate the failure modes and `into_panic()` recovers the payload. A panicking task does not kill the runtime — the panic is caught at the task boundary, so joining is the only way to see it at all.
- `JoinSet<T>`: tasks run whether or not you poll it, `join_next()` yields in **completion order** and is cancel-safe, results accumulate until drained, and **dropping the set aborts every task in it**. `abort_all()` aborts without draining; `shutdown()` does both. `TaskTracker` (tokio-util) instead discards outputs and drops tasks as they exit, so it cannot grow: `close()` then `wait()`, which returns only once the tracker is both closed and empty. **`close()` is not an admission barrier** — spawning into a closed tracker still works — so stop the producers first, or `wait()` may never settle.

## Cancel safety

Futures are cancelled by **dropping** them; tasks are cancelled by **`abort()`**. Awaiting a future exposes it to every enclosing `select!` and `timeout`; spawning it insulates it, which is the deliberate way to drive a cancel-unsafe operation to completion. **Cancel safety is local**: dropping this future and recreating it is a no-op. **Cancel correctness is global and yours**: half a protocol exchange dropped between the request write and the response read is built entirely from cancel-safe parts and is still a bug.

| Class | Futures |
| --- | --- |
| Cancel-**safe** | `mpsc::Receiver::recv`, `mpsc::UnboundedReceiver::recv`, `broadcast::Receiver::recv`, `watch::Receiver::changed`, `TcpListener::accept`, `UnixListener::accept`, `signal::unix::Signal::recv`, `AsyncReadExt::read` and `read_buf`, `AsyncWriteExt::write` and `write_buf`, `StreamExt::next` (tokio-stream and futures), `JoinSet::join_next`, `CancellationToken::cancelled`, `Interval::tick` |
| Cancel-**unsafe**, loses data | `AsyncReadExt::read_exact`, `read_to_end`, `read_to_string`, `AsyncWriteExt::write_all`, `mpsc::Sender::send`, `SinkExt::send` |
| Cancel-**unsafe**, loses queue position | `tokio::sync::Mutex::lock`, `RwLock::read`/`write`, `Semaphore::acquire`, `Notify::notified` |

`mpsc::Sender::send` deserves its mechanism spelled out, because the label alone invites a retry loop that silently drops requests: if another `select!` branch wins, the message was definitely **not** sent — and the value you passed in was **dropped with the future**, and the sender lost its place in the fairness queue. Fixes for that row: split `send` into `reserve()` (cancel-safe) plus an infallible `Permit::send`; keep ownership of the value outside the cancellable future and define the retry explicitly; use `write_all_buf` with a cursor so partial progress is kept; or move the operation into a task that owns the stream for its whole life.

`select!` runs all branches on one task, so they never run simultaneously and may borrow shared data. The first branch to complete **with a matching pattern** wins and the rest are dropped. Poll order is random by default, which buys some fairness; `biased;` makes it top-to-bottom and hands starvation prevention back to you — use it so a shutdown branch beats an always-ready data branch, never to quiet a flaky test. A `, if cond` branch is not polled when disabled, but **its async expression is still evaluated**, so keep those expressions cheap and free of side effects or build them conditionally outside the macro. If every branch is disabled with no `else` the macro **panics**: supply the `else`. `try_join!` drops its siblings mid-operation on the first error. A future built inline in a `select!` loop is rebuilt each iteration and its partial progress vanishes — build once, pin, select on `&mut`:

```rust
let operation = action();
tokio::pin!(operation);
loop {
    tokio::select! {
        _ = &mut operation => break,
        Some(v) = rx.recv() => if v % 2 == 0 { break },
    }
}
```

`CancellationToken` (tokio-util): `cancel()` cancels the token and all descendants. `clone()` links **bidirectionally**; `child_token()` links **downward only**, and a child of an already-cancelled parent starts cancelled. That asymmetry is the design — one root per process, a child per subsystem, a grandchild per session. Propagation is **not atomic**: while `cancel()` is still running, one observer can see child A cancelled and child B not, so never make a cross-task invariant depend on simultaneous observation — join, acknowledge, or use a tracker. `run_until_cancelled(fut) -> Option<T>` is the terse shutdown select; `drop_guard()` cancels the children if the parent task unwinds.

## Shutdown is two phases and a deadline

Signal, then wait — and the wait needs a deadline, or shutdown hangs on the one task that ignored the signal.

```rust
token.cancel();
tracker.close();
match tokio::time::timeout(GRACE, tracker.wait()).await {
    Ok(()) => Ok(()),
    Err(_) => { tasks.abort_all(); while tasks.join_next().await.is_some() {} Err(Error::ShutdownTimeout) }
}
```

Without tokio-util, phase two is: hold an `mpsc::Sender` clone in every task and `recv().await` on the receiver, which returns `None` only once every sender has dropped.

Dropping a `Runtime` cancels ordinary tasks at their next yield point, but **waits without a deadline for every already-started `spawn_blocking` closure**, which always runs to its own return. `shutdown_timeout(d)` bounds how long you wait — it does not stop the blocking work, which keeps running and keeps holding whatever it holds. `shutdown_background()` exists to avoid the panic from dropping a runtime inside async code, with the same caveat. So a service with blocking work needs that work to be externally stoppable and bounded; runtime drop is not a shutdown plan.

## Sync under await

`std::sync::Mutex` is the default in async code — lock, mutate, drop the guard, never await while holding it. Its guard is `!Send`, so holding it across `.await` makes the future `!Send` and the compiler names the guard; that error is the diagnostic, not the obstacle. But the compiler only catches the `Send` case: a guard held across an await inside a `spawn_local` or single-task context compiles and deadlocks.

Fixes in order: scope the guard so it drops before the await (`har-concurrent` owns the `if let` temporary-lifetime trap that defeats this), move the state into a task and message it, and only then `tokio::sync::Mutex` — which tokio's own docs call more expensive and recommend against for plain data. It is FIFO, does not poison, has a cancel-unsafe `lock()`, and its `blocking_lock()` panics in an async context. Switching mutex kind never fixes contention; shard, or move to a task. Reach for the async mutex when the guard genuinely must span an await over an IO-like shared resource, not to silence a `!Send` error.

Two other things must not cross an await: a tracing `Span::enter` guard (`har-layout` owns that rule), and cleanup that needs to await — `Drop` is synchronous, so such a type gets an explicit `async fn shutdown(self)` with `Drop` only reporting that it was skipped.

| Need | Primitive |
| --- | --- |
| One value, one producer → one consumer | `oneshot` — the reply channel; prefer a `JoinHandle` if the value is the task's final act |
| Many values, many producers → one consumer | `mpsc::channel(n)` — `send().await` **is** the backpressure; `recv` is cancel-safe |
| Every consumer sees every value | `broadcast` — ring buffer; a slow receiver gets `Lagged`, it does not stall producers |
| Latest value only: config, shutdown flag | `watch` — stores only the most recent value; `changed()` is cancel-safe |
| Wake a task, no data | `Notify` — one permit is stored, so `notify_one` before `notified()` is not lost |
| Cap concurrency at N | `Semaphore` — FIFO; `Arc<Semaphore>` + `acquire_owned()` moves a permit into a task; `close()` wakes every waiter |
| Read-mostly state held across an await | `tokio::sync::RwLock` — writer-preferring, both halves cancel-unsafe |
| Never | `unbounded_channel` — a memory leak with a queue-shaped API: a consumer 2× slower than its producer grows without bound and fails as an OOM, not a queue |

## Pin, practitioner dose

An `async fn` compiles to a state machine that becomes self-referential once polled — a local borrowed across an `.await` is a pointer into the future's own storage, so moving it after the first poll invalidates that pointer. `Pin<P>` says the pointee stays valid at that address until its `drop` runs; `Unpin` is an auto trait that cancels the restriction, and async blocks and hand-written futures are `!Unpin`. Fix with `std::pin::pin!` / `tokio::pin!` on the stack, or `Box::pin` when the future needs a `'static` home, a struct field, or a slot in a collection. You meet it in exactly five places: reusing a future across `select!` iterations, storing a future in a struct, `Pin<Box<dyn Future + Send>>` as a return type, `Sleep` in a loop, and implementing `Future` or `Stream` by hand. (`har-unsafe` owns the structural-pinning obligations if you project through one.)

## Actors

A task owns the state and a `Receiver`; a `Clone`able handle owns only a `Sender`. Nothing is shared, so no guard exists that could be held across an await.

```rust
enum Message { Id { reply: oneshot::Sender<Id> } }

struct Handle { tx: mpsc::Sender<Message> }

impl Handle {
    async fn id(&self) -> Result<Id, Error> {
        let (reply, rx) = oneshot::channel();
        self.tx.send(Message::Id { reply }).await.map_err(|_| Error::ActorGone)?;
        rx.await.map_err(|_| Error::ActorGone)
    }
}
```

The run loop is `async fn run(mut self)` called **inside** the spawn, never a method that spawns internally — spawn needs `'static` and a `&mut self` method borrows. Never merge handle and actor into one struct; that hands every handle the actor's fields. Shutdown is ownership-driven: drop the last handle, `recv()` returns `None`, the loop exits, its `JoinHandle` resolves. Backpressure is the bounded channel. It beats `Arc<Mutex<State>>` for a stateful component because invariants spanning several fields need no lock order, the shutdown edge is free, and the message enum is a reviewable API. It costs a round trip per call, and deadlocks if two actors await each other's replies.

# Subprocess

## The pipe-fill deadlock

Pipes are an OS mechanism; these rules apply to `std::process::Command` on a thread exactly as they apply to `tokio::process`. A pipe is a fixed-size kernel buffer whose size varies by target, so never encode a number. When it fills the writer blocks, and if the parent is blocked writing stdin while the child is blocked writing stdout, neither ever moves — no error, no timeout, forever. **Every captured pipe needs a drainer that is not on the same sequential path as any other pipe**: one task or thread per stream, or a `select!` polling all of them, or `Stdio::inherit()` / `Stdio::null()` so the pipe does not exist. Capturing stderr and reading only stdout is the same deadlock with a quieter trigger — the child runs fine until it gets chatty. Inheriting stderr and draining stdout on its own thread is the correct shape for a long-lived protocol peer. `.take()` below moves each handle out so a reader can own it while the parent keeps `Child` for `wait()` / `kill()`.

```rust
let mut child = Command::new(bin)
    .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit())
    .spawn()?;
let stdin = child.stdin.take().ok_or(Error::NoStdin)?;
let stdout = child.stdout.take().ok_or(Error::NoStdout)?;
```

## Exit is not EOF

| Signal | Means | Does not mean |
| --- | --- | --- |
| a read returns `Ok(0)`, or the stream yields `None` | the child closed stdout | the child exited — it may still be running |
| `wait()` returns `ExitStatus` | the process is gone and reaped | all output has been read |

`wait()` closes the child's stdin before waiting, to help avoid exactly this deadlock — but only the stdin still inside the `Child`. Once you `.take()` it, **you** send EOF by dropping it (async: `shutdown().await`, then drop). A child that reads to EOF and never gets one hangs, and `wait()` will not save you. `try_wait()` neither drops stdin nor wakes the task on exit, so polling it is a busy-wait that also never sends EOF. Order for a protocol peer: drop stdin → drain stdout to EOF → `wait()`; any other order loses the tail or hangs. `wait_with_output()` does all three for a short-lived child whose output fits in memory, and is wrong for a long-lived peer.

## Kill, reap, and groups

A spawned process **keeps running after its `Child` is dropped**. `kill_on_drop(true)` changes that, but `Drop` cannot await, so it cannot reap: tokio makes a best-effort background attempt with no timing or completion guarantee, and on Unix the child can stay a zombie — and nobody reaps it at all if the runtime is gone first. Treat it as a backstop for panic and early-return paths, never as the shutdown plan. The plan is `start_kill()?` — the synchronous half, callable from `Drop` — followed by an awaited `wait()`, or `kill().await`, which is SIGKILL plus `wait`. Killing the child kills the child: a shell wrapper, an `npx`, or a runtime that forks workers leaves grandchildren holding your mounts and fds. `Command::process_group(0)` (Unix, std since 1.64) makes the child its own group leader; then signal the group — `SIGTERM`, grace, `SIGKILL`, `wait()`. Never skip the `wait()` or you have a zombie whichever signal you sent. `libc::killpg` is an `unsafe` call and cannot live under a workspace `forbid(unsafe_code)`; the safe routes are `nix::sys::signal::killpg` or `command-group` (`har-supply` owns whether to add either). Second effect: a child in its own group no longer receives the terminal's Ctrl-C, so you must forward termination yourself.

## Across a transport that is not a pipe

When the peer's stdio is bridged over a socket, a vsock, a VM boundary or an RPC transport, there is no `Child` on your side at all — `kill_on_drop`, `wait`, `ExitStatus` and `killpg` are unavailable, process lifecycle becomes a protocol concern, and exit status must be carried in-band. EOF stops being free: closing a pipe write end is an OS-guaranteed EOF, while over a bridge it is whatever the bridge propagates, so the half-close must be explicit or in-band. The failure mode inverts — a pipe deadlocks silently, a socket can be reset, half-open, or black-holed — so a bridged stream needs a drainer **and** a heartbeat, because "stuck" and "slow" are otherwise indistinguishable. Framing stops being tidiness and becomes mandatory: a bridge re-chunks freely.

# Framing

`read` fills a buffer with whatever bytes have arrived. It is not a message: one write can arrive as three reads and three writes as one read, and every stdio protocol that "worked in testing" got one JSON object per read on localhost and half an object in production. A `Decoder` makes mishandling a partial read unrepresentable, because the only two shapes it can return are `Ok(None)`, not enough yet, and `Ok(Some(frame))`, exactly one; buffer management, leftover bytes, multi-frame arrivals and retries all live inside `Framed`. `decode_eof` is where you decide whether a trailing partial frame is an error or a final message, and `Encoder::encode` **appends** to the destination buffer.

| Ask | Codec |
| --- | --- |
| Newline-delimited JSON-RPC over stdio (jsonl) | `LinesCodec::new_with_max_length(n)` |
| Payload can contain a raw `\n`, or is binary | `LengthDelimitedCodec` — `u32` big-endian prefix, **8 MiB max frame by default**, `builder()` for other shapes |
| Header block then body (`Content-Length:`, LSP style) | hand-rolled `Decoder` — nothing built-in covers it |
| Pure proxying | `BytesCodec`, or `copy_bidirectional` and no codec |
| A real format (protobuf, CBOR) | length-delimited frame plus your parser; do not merge the two |
| **No codec crate admitted** | `Read::take(max)` plus `BufRead::read_until(b'\n', ..)`, in a loop that owns the reader — `har-threat`'s resource-bounds section writes the whole function, including the bounded resynchronize the naive version gets wrong and the reason it reads bytes rather than a `String` |

**Always construct the bounded form.** `LinesCodec::new()` puts no upper bound on a line, which on a peer-fed stream is an unbounded allocation; `new_with_max_length` resynchronizes instead, discarding up to the limit until the next newline. `max_frame_length` is the same knob for length-delimited frames. `har-threat` owns why a peer will make you regret the missing bound. `Framed` is both `Stream` and `Sink`, so one `&mut` does one at a time. For a duplex protocol use `StreamExt::split()` (or `tokio::io::split`) and give each half its own task, joined by channels. `SinkExt::send` inside a `select!` cannot corrupt the wire but does lose the message; a dedicated writer task fed by a channel makes the question disappear.

# Streams

`Stream` is the async `Iterator`. **It is still not in std**: `core::async_iter::AsyncIterator` is nightly-only behind `feature(async_iterator)`, and its tracking issue — rust-lang/rust#79024 — is open with the implementation, documentation and stabilization steps all unchecked. The trait you use is `futures_core::Stream`, re-exported by `futures` and `tokio-stream`.

```rust
async fn pump(io: impl AsyncRead + Unpin, out: mpsc::Sender<Frame>) -> Result<(), Error> {
    let mut lines = FramedRead::new(io, LinesCodec::new_with_max_length(MAX_FRAME));
    while let Some(line) = lines.next().await {
        out.send(serde_json::from_str(&line?)?).await.map_err(|_| Error::Closed)?;
    }
    Ok(())
}
```

`None` is EOF — the peer closed its write half — and is normal termination, not an error. Each item is its own `Result`, so a decode error is per-frame and the protocol decides skip or abort. The `send(..).await` is the backpressure, and the loop owns the reader for its whole life, so no cancel-safety question arises. This shape replaces reading inside a `select!`. Adapters worth knowing: `merge` and `chain` to combine, `throttle` to rate-limit, `timeout` for a per-item deadline, `chunks_timeout(n, dur)` to batch up to n items or until a deadline, and `futures`' `buffer_unordered(n)` / `for_each_concurrent(n, f)` when items must run concurrently but at most N at a time. `ReceiverStream` bridges a channel into all of them. Backpressure survives a chain only if nothing in it buffers without bound. A `Framed` read half applies it for free — nobody polls, the socket buffer fills, the peer's write blocks. A bounded `ReceiverStream` keeps it, the unbounded one destroys it, `for_each_concurrent(None, ..)` bounds nothing, and spawning a task per item removes backpressure entirely since the spawn always succeeds — bound that with a `Semaphore`. A render or UI thread must never be the backpressure point: feed it through a bounded channel with a coalescing policy (`watch` for latest-wins, `chunks_timeout` to batch) so a chatty producer slows the reader task, never the frame.

# Time

`timeout(dur, fut) -> Result<T, Elapsed>`. Two independent traps.

**`Err(Elapsed)` does not mean nothing happened.** It means the inner future was dropped at whatever await point it had reached — cancel safety again, imposed by a timer instead of by `select!`.

- `timeout(d, write_all(buf))` may have written a prefix. The peer is mid-message and the stream is unusable: tear the connection down, never retry on it.
- `timeout(d, read_exact(..))` may have consumed bytes that are now gone.
- `timeout(d, framed.next())` is safe — the codec buffer keeps the partial frame — but the timer is gone, so a naive retry restarts the full duration.

**`Ok(..)` does not mean it finished in time.** `timeout` polls the inner future *before* checking the deadline, and can only drop a future at an await point. A future that runs a CPU loop or a blocking call without yielding runs to completion, overruns the duration by however long it took, and returns `Ok`. `timeout` is therefore not enforcement: it is a deadline on a cooperating, yielding operation. Hard enforcement needs the work moved to a cancellable boundary — its own task, its own thread with a stop flag — plus a protocol-level deadline or watchdog.

So wrap operations whose abandonment is safe — a whole request/response, a `Framed::next`, a connect, a handshake. For a write, put the writer in its own task and time out on the response or on the task's `JoinHandle`. **A deadline composes; a duration does not.** Compute `let deadline = Instant::now() + budget;` at the entry point and pass the `Instant` down to `timeout_at` at each step, or N steps each get the full budget. Per-operation beats one global timeout on three counts: it names which stage hung, the only version actionable in a log; an inter-frame timeout reset on every frame distinguishes "slow but progressing" from "wedged", which a global one cannot; and recovery differs per stage — a connect timeout is retryable, a mid-response timeout means the protocol state is unknown and the session must be dropped. `har-threat` owns that a peer-controlled wait needs a bound at all. `timeout` is imposed from outside and drops the future mid-flight; a `CancellationToken` is observed from inside and lets the task run its own cleanup, so anything with an invariant to restore wants the token.

`sleep` once, `timeout` for a deadline on an existing future, `interval` for periodic work. `interval`'s first `tick()` completes immediately — right for a poller, wrong for a grace timer — and `tick` is cancel-safe. `MissedTickBehavior` applies when a deadline passed while you were busy, and only when the delay exceeds 5 milliseconds:

| Variant | Behavior | Right when |
| --- | --- | --- |
| `Burst` (**default**) | ticks as fast as possible until caught up | the work is cheap and catching up is the point |
| `Delay` | ticks at multiples of `period` from when `tick` was called | you want a guaranteed minimum gap: polling, anything with a per-run cost |
| `Skip` | skips missed ticks, staying aligned to the original schedule | a missed run is worthless: a UI refresh, a metrics sample |

`Burst` is the default for backwards compatibility, and after a stall — a slow disk, a suspended laptop — it fires every missed tick back to back. **Choose the variant explicitly on every `interval` that drives IO.** `Sleep` is `!Unpin`: pin it, and use `Sleep::reset` to re-arm inside a `select!` loop. (`har-io` owns clock choice and wall-clock anomalies.)

# Testing async

`#[tokio::test]` defaults to `current_thread`. Keep it — faster, more deterministic, and the only flavor where the clock can be paused. Reach for `flavor = "multi_thread"` only when the test is *about* parallelism or the code calls `block_in_place`. `tokio::time::pause()` freezes tokio's `Instant` (not `std`'s), and while paused with no work to do the runtime **auto-advances the clock to the next pending timer** — so a test of a 30-second timeout finishes in microseconds with no `advance` call at all. Use `advance(dur)` explicitly only to observe state between two timers, e.g. asserting two of five retries happened, or to make a `MissedTickBehavior` choice directly observable.

```rust
#[tokio::test(start_paused = true)]
async fn handshake_gives_up() {
    let (client, _server) = tokio::io::duplex(64);
    assert!(timeout(Duration::from_secs(30), handshake(client)).await.is_err());
}
```

`tokio::io::duplex(cap)` with a **small** capacity is the workhorse: a real in-memory `AsyncRead + AsyncWrite` pair that reproduces "the writer blocks because nobody drains" in a few lines, the closest thing to a unit-testable pipe-fill deadlock. Test a cancellation path one of three ways, cheapest first: build the future, poll it once (`timeout(Duration::ZERO, fut)` or `futures::poll!` on a pinned future), drop it, and assert the observable state — bytes consumed, item taken, lock released; or elapse a `timeout` on a paused clock and assert the object is in the state you documented; or drive the real token path and assert cleanup ran and the `JoinHandle` resolved rather than detached. `yield_now()` is a nudge documented to give no scheduling guarantee, so a test whose pass depends on it is flaky by construction — use a channel, a `Notify`, or a `Barrier`. Deterministic simulation (`turmoil`, `madsim`) pays only when the bug class is message ordering, partition, or retry-race across two or more independent peers and the interleaving is unreproducible; it pays nothing if any dependency reads time or entropy outside the runtime, which destroys the determinism that was the whole point. `har-verify` owns where that sits on the ladder, and owns `shuttle` for randomized async schedules.

# Traps

- A future built and never polled: no work happens, no error appears. `join!`/`select!` mistaken for parallelism — one task, one thread, one blocking branch stalls the rest.
- Writing stdin while awaiting output or exit; or capturing stderr and never draining it — the same deadlock, quieter. One drainer per pipe.
- stdin `.take()`n and never dropped: `wait()` can no longer close it, and the child waits for an EOF that never comes. `try_wait` expected to behave like `wait` is the same bug in a loop.
- `kill_on_drop` believed to reap — it kills, and the reaping is somebody's best effort. Killing the child instead of the group leaves grandchildren holding your resources.
- `read_exact` / `read_line` / `write_all` / `read_u32` inside a `select!` — documented cancel-unsafe, and the losing branch takes the bytes with it. `SinkExt::send` there loses the whole message, and `mpsc::Sender::send` drops the value you handed it.
- A future built inside a `select!` loop: rebuilt every iteration, partial progress silently discarded. A disabled `, if cond` branch whose expression still runs.
- Reachable panics of the runtime: `select!` with every branch disabled and no `else`, `blocking_lock()` in async context, `block_in_place` on a current-thread runtime, tokio types without `enable_all()` (that one fails at runtime, not compile time).
- `biased;` added to fix flakiness — it changed fairness, not the bug. `abort()` treated as immediate, or as a barrier — it returns before the task has stopped.
- Detached tasks: a dropped `JoinHandle` still runs, its panic is silent, shutdown does not wait for it. `TaskTracker::close()` mistaken for an admission barrier.
- Runtime drop expected to be prompt: it waits, without a deadline, for every started `spawn_blocking` closure.
- `Err(Elapsed)` on a partial write treated as "nothing happened", or `Ok(..)` from a `timeout` treated as proof the deadline held.
- `interval` bursting after a stall, or its first tick firing immediately where a delay was intended.
- `std::fs` inside an async fn. `tokio::fs` is `spawn_blocking` under the hood, so for a batch, one `spawn_blocking` around `std::fs` beats N `tokio::fs` calls.
- `std::process::Command` inside an async fn by accident — `.output()` / `.status()` / `.wait()` block the worker and the `use` line is the only difference. Grep for it in async modules as a review step.
- `BufWriter` dropped without `flush()`: data discarded, and `Drop`'s flush swallows the error (`har-hot-path` owns buffering for throughput).
- `next()` on an unpinned stream — a compile error, fixed by `tokio::pin!`. An unbounded channel anywhere in a stream chain — backpressure silently gone, no error at all.
- `async fn` in traits is stable since Rust 1.75 (December 2023), but such traits are **not dyn-compatible** and the returned future carries no `Send` bound. For `Box<dyn Handler>` use `#[async_trait]`; for static dispatch that must spawn, `#[trait_variant::make(Trait: Send)]` or a hand-written `-> impl Future<Output = T> + Send`. (`har-api` owns the public-API commitment that creates.)
- Async closures are stable since Rust 1.85 (2025-02-20), with `AsyncFn`/`AsyncFnMut`/`AsyncFnOnce` in `std::ops` **for use as bounds**; `async || {}` can lend borrows of captured state that `|| async move {}` cannot. Call them with ordinary call syntax — the traits' `async_call*` methods are still nightly-only behind `async_fn_traits`. Below that MSRV, `impl Fn() -> impl Future` stands.
