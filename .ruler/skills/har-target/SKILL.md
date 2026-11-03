---
name: har-target
description: Rust beyond the host — no_std and embedded crates, panic handlers and allocators, interrupts and critical sections, target atomic capability, cross-compilation contracts, WebAssembly feature sets and host imports, const generics and compile-time evaluation. Load when a crate must build for a target that is not the machine you are on.
---

# Targets

A crate that compiles for the host is a crate that works on one target. Everything here is about the rest of them, and the first rule is the one agents break most often: **`cargo build --target …` succeeding is not validation**. Capabilities are a contract, and the contract is not inferred from the host.

## The target contract

Write one down per shipped triple, before the first cross build:

| Term | Question it answers |
| --- | --- |
| triple and target spec | which `--target`, and is it tier 1, 2, or custom |
| `std` / `alloc` / `core` | which of the three the crate may use |
| panic policy | `unwind`, `abort`, or a `#[panic_handler]` you supply |
| allocator | a global allocator, a bounded pool, or no allocation at all |
| atomics | which widths exist — `target_has_atomic` — and whether a lock-based fallback ships |
| entropy | is there a reviewed CSPRNG source, or is this target unsupported (`har-io`) |
| clock | is there a monotonic clock, and what is its resolution |
| filesystem, network | present, absent, or emulated with different semantics |
| linker, sysroot, ABI | what links it, and against what |
| CPU or Wasm features | the minimum feature set, and how it is enforced |
| host imports | what the embedding runtime must provide |

CI builds **and runs** the test suite on every supported target or its emulator. A target you only compile for is a target you are guessing about.

## no_std

`#![no_std]` means the crate does not link `std`. It does **not** mean the crate cannot allocate — `extern crate alloc;` brings `Box`, `Vec`, `String` and the collections back, backed by whatever global allocator the final binary installs. Those are two separate decisions:

| Crate needs | Declare |
| --- | --- |
| No OS, no allocator | `#![no_std]` — `core` only |
| No OS, heap available | `#![no_std]` + `extern crate alloc` |
| An OS | plain `std` |

For a library that could serve both, make `std` an **additive default feature** that adds capability, never one that removes it:

```toml
[features]
default = ["std"]
std = ["alloc", "thiserror/std"]
alloc = []
```

```rust
#![cfg_attr(not(feature = "std"), no_std)]
#[cfg(feature = "alloc")]
extern crate alloc;
```

`cargo build --no-default-features` is then a required CI job, not an afterthought — Cargo unifies features across the graph, so one dependency enabling `std` re-enables it for everyone (`har-api` owns additivity).

What disappears with `std`: threads, the filesystem, networking, `SystemTime`, OS entropy, `std::error::Error` in older toolchains, `HashMap`'s default hasher and its random seed, and backtraces. Anything in `har` that assumed a heap or an OS needs a bounded replacement — fixed-capacity collections (`heapless`) rather than `Vec`, a `Result` rather than an allocation that cannot fail.

## Bare metal

A final `no_std` binary supplies what `std` used to:

- **Exactly one `#[panic_handler]`**, and it is a product decision, not boilerplate: halt, reset, log to a debug channel, or record a crash reason to non-volatile storage first. `panic = "abort"` in the profile, since there is nothing to unwind into.
- **One `#[global_allocator]`, or none.** Prefer none: allocation on a device is a failure mode that arrives at 3am under memory pressure. Where it is unavoidable, make it bounded and explicit, and decide what allocation failure does.
- Startup, memory layout, and the linker script are part of the build and belong under review like any other source. `cortex-m-rt` and friends generate most of it; the `memory.x` is still yours.
- `#[no_main]` plus the runtime's entry attribute replaces `fn main`.

### Interrupts are an execution context

Treat an interrupt service routine the way you treat a thread — because it is one, with extra rules:

- Anything an ISR touches is shared state. Reentrancy is real: the same ISR can preempt itself if it re-enables interrupts.
- A **critical section** that disables interrupts protects against interrupts **on that core only**. On a multi-core part it is not mutual exclusion, and reasoning that works on a single-core MCU silently breaks on a dual-core one. Use an SMP-capable primitive there.
- Keep critical sections bounded and short — they raise worst-case latency for everything else.
- An ISR must not allocate, must not block, and must not panic into a handler that assumes a working world.
- `critical-section` and `embassy-sync` are the portable vocabulary for this; hand-rolled `cli`/`sei` pairs are how the SMP bug gets written.

### Atomics are target-conditional

There is no promised lock-based fallback for a width the target lacks. Gate on capability and ship the alternative:

```rust
#[cfg(target_has_atomic = "32")]
use core::sync::atomic::AtomicU32 as Counter;
#[cfg(not(target_has_atomic = "32"))]
use crate::critical_section_counter::Counter;
```

`portable-atomic` packages that fallback for the common cases. `har-concurrent` owns the ordering rules once you have the atomic.

## Cross-compilation

- Add the target and any component explicitly (`rustup target add`), and pin them like the toolchain (`har-supply`).
- Configure the linker and sysroot per target in `.cargo/config.toml`, not in an environment variable someone has to remember.
- `cfg!(target_os = ..)` inside a build script tests the **host**. Build scripts read `CARGO_CFG_TARGET_OS`, `CARGO_CFG_TARGET_ARCH`, `CARGO_CFG_TARGET_HAS_ATOMIC` and friends instead — this is a common and silent cross-compilation bug.
- A build script's own dependencies are compiled for the host and its output for the target. Keep them separate in `Cargo.toml`; do not let a build dependency leak into the target graph.
- Run the tests under an emulator (`cross`, `qemu`, a device runner) rather than declaring compilation success.
- `-C target-cpu` and `#[target_feature]` are part of the contract: calling a `#[target_feature]` function on a CPU without it is UB (`har-unsafe`). Distributed binaries take a conservative baseline plus runtime detection (`har-hot-path`).

## WebAssembly

Wasm is a family of targets with different capabilities, and picking the wrong one produces a binary that builds and then traps.

| Target | For |
| --- | --- |
| `wasm32-unknown-unknown` | a browser or a host that supplies its own imports; most OS-backed `std` calls fail at runtime |
| `wasm32-wasip1` / `wasm32-wasip2` | a WASI host that provides files, clocks, and entropy through the interface the version names |
| `wasm32v1-none` | `no_std` deployments on a runtime that supports only Wasm Core 1.0 plus mutable globals |

- **Pick a minimum feature set and verify it against the deployed runtime.** LLVM's default proposals can exceed what an embedder implements, and the failure is a validation error at instantiation, not a compile error.
- On `wasm32-unknown-unknown`, `SystemTime`, the filesystem, threads and OS entropy are not there. Randomness needs a registered source (`har-io`); time needs a host import.
- Atomics need both a target that enables them and a runtime providing shared memory. Gate them on both.
- Rebuilding `std` (`-Z build-std`) to change these assumptions is nightly-only — that is a separately pinned lane (`har-supply`), never a bootstrap escape in the production build.
- Enumerate the imports your module requires. A host that does not provide one fails at instantiation, so the import list is part of the shipped contract and belongs in a test.

## Const generics and compile-time evaluation

Fixed sizes are where embedded and protocol code meets the type system, and where it is easy to overreach.

- Use a const parameter when the value genuinely is part of the type's identity — a buffer capacity, a protocol field width, a matrix dimension — and when knowing it at compile time **deletes a runtime state** rather than merely documenting one.
- A size that arrives from outside the process is not compile-time. Give it a fallible runtime constructor; `har`'s checked-constructor pattern applies unchanged.
- Stable const parameters are limited to the integer types, `char` and `bool`. `generic_const_exprs` — arithmetic in const parameter positions — is **unstable**, and it does not belong in a crate with a stable MSRV. Isolate any approved nightly feature behind its own pinned toolchain and target matrix (`har-api` owns the public-API commitment a const parameter creates).
- Test at boundary values (`0`, `1`, `MAX`) and on each supported target width. A `const N: usize` behaves differently on a 16-bit target than on the 64-bit host you wrote it on.
- Keep `const fn` bodies deterministic and free of I/O intent, and do not assume anything about addresses. **Overflow or an out-of-bounds index in a const context is a compile error**, while the same expression at runtime warns and then panics — so a const-evaluated path and its runtime twin have different failure behaviour, and that difference is part of the API contract.

Const evaluation is not a general build-time execution environment. When you need real code generation, that is a build script, with everything `har-supply` says about build scripts being privileged code.

## Checklist

- A written target contract per shipped triple, and CI that builds **and runs** on each
- `no_std` and `alloc` chosen separately; `std` is an additive feature, and `--no-default-features` is a CI job
- Exactly one `#[panic_handler]`, chosen deliberately; allocator present and bounded, or absent
- Linker script and memory layout reviewed as source
- ISRs treated as concurrent contexts; critical sections bounded, and SMP-capable where the part has more than one core
- Every atomic gated on `target_has_atomic`, with a shipped fallback
- Build scripts read `CARGO_CFG_*`, never host `cfg!`
- Wasm: target variant chosen, minimum feature set verified against the runtime, import list tested
- Entropy, clock and filesystem availability confirmed per target, not assumed (`har-io`)
- Const generics only where they delete a runtime state; no unstable const features in a stable MSRV build
