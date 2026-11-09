---
name: har
description: High-assurance Rust patterns — newtypes, typestate, exhaustive enums, Result discipline, no panics on reachable paths, checked arithmetic, safe indexing. Load when writing or reviewing Rust that must not crash.
---

# High-assurance Rust: core patterns

Written against **stable Rust 1.99.1**. A recent API or lint carries its stabilization version in parentheses; if that version is above your `rust-toolchain.toml` pin, you cannot use the item yet.

## Panic budget

A panic on a reachable path is a bug. Every source has a non-panicking form:

| Panics | Use |
| --- | --- |
| `v[i]`, `v[a..b]` | `v.get(i)`, `v.get(a..b)` |
| `map[&k]` (`Index` on `HashMap`/`BTreeMap`) | `map.get(&k).ok_or(Error::Missing)?` |
| `&s[..n]` on a `str` — panics mid-character | `s.get(..n)`, or a char/grapheme boundary (`har-io`) |
| `a + b`, `a - b`, `a * b` | `checked_*`, `saturating_*`, `wrapping_*` |
| `a.strict_add(b)` and the rest of the `strict_*` family | they panic on overflow in **every** profile — that is their purpose; `checked_*` instead |
| `a / b`, `a % b` — including `i32::MIN / -1`, which panics in **release** too | `checked_div`, `checked_rem`, or type the divisor `NonZeroU32` |
| `a << b`, `a >> b` with `b >= bits` | `checked_shl`, `checked_shr`, or mask the shift |
| `i32::MIN.abs()`, `.neg()` | `checked_abs`, `unsigned_abs` |
| `.sum()`, `.product()` over values that can overflow | `try_fold` with `checked_add` |
| `.unwrap()`, `.expect()` | `?`, `.ok_or(Error::Missing)?`, `unwrap_or_default()` |
| `panic!`, `unreachable!`, `todo!` | return `Err` |
| `dst.copy_from_slice(src)` | length check first, or `dst.iter_mut().zip(src)` |
| `chunks(0)`, `windows(0)`, `step_by(0)` | validate the size into a `NonZeroUsize` — since 1.99 `unconditional_panic` catches the literal case, which is the easy half |
| `RefCell::borrow_mut` | `try_borrow_mut()?`, or restructure to `&mut` |
| `Vec::remove`, `Vec::insert`, `split_at`, `drain(bad_range)` | `get`-guarded index, `split_at_checked` |
| `String::insert`, `String::remove` off a char boundary | operate on `char_indices`, or build a new `String` |
| `Duration::from_secs_f64(x)` with negative, NaN, or overflowing `x` | `Duration::try_from_secs_f64(x)?` |
| `Instant + Duration` past the representable range | `checked_add` |
| `Mutex::lock().unwrap()` on a poisoned lock | decide the policy (`har-concurrent`) |
| unbounded recursion | iterate with an explicit stack |

`unwrap`/`expect` are allowed in tests, benches, and build scripts. Nowhere else.

`assert!` in a constructor kills the caller's process over the caller's mistake. Return `Result`:

```rust
// Panics on a caller's bad input, from inside a library, with no recovery.
pub fn new(width: u32, height: u32) -> Self {
    assert!(width > 0 && height > 0, "dimensions must be non-zero");
    Self { width, height }
}

// The same check, handed back to whoever can do something about it.
pub fn new(width: u32, height: u32) -> Result<Self, Error> {
    let width = NonZeroU32::new(width).ok_or(Error::ZeroWidth)?;
    let height = NonZeroU32::new(height).ok_or(Error::ZeroHeight)?;
    Ok(Self { width, height })
}
```

Keep `assert!` only for invariants no caller can influence — a postcondition of your own arithmetic, a table whose length you control. `debug_assert!` for anything whose cost matters and whose failure a test would catch.

## Result discipline

- Fallible public fn returns `Result<T, CrateError>`. Propagate with `?`.
- `let _ = fallible()` drops an error. Handle it or propagate it.
- One error variant per distinct caller reaction. Finer is noise; coarser strips the caller's ability to respond.
- Variants carry the offending value and the bound, not a rendered string:

```rust
enum Error { KeyTooShort { len: usize, min: usize }, KeyTooLong { len: usize, max: usize } }
```

Two exceptions to "carry the value": never a secret — carry the length and the bound instead (`har-threat`) — and never something unbounded that came off the wire, since the error gets logged.

- `Err(())` is fine inside a crate, never across a public API.
- `Option` means absence is normal. `Result` means the operation failed. Never encode failure as `None`.
- An infallible signature wrapping a fallible operation is the defect: `fn load() -> Config` that aborts on failure gives the caller nothing to handle.
- `#[must_use]` on functions returning `Option`, and on types whose discard loses the only evidence of an unfinished operation. `Result` already carries it. (`har-api` owns the type-level rule.)
- A `Result` is not the whole outcome when the operation crossed a process boundary: a timeout means *unknown*, not *failed*. `har-io` owns that shape.

## Make invalid states unrepresentable

Newtype every id and every unit-bearing scalar. `struct FrameId(u64);` cannot be passed where `GateId` is expected, at zero runtime cost.

Validate once at construction; from then on the type is the proof:

```rust
pub struct Key(Vec<u8>);
impl Key {
    pub fn new(bytes: Vec<u8>) -> Result<Self, Error> {
        match bytes.len() {
            5..=256 => Ok(Self(bytes)),
            n => Err(Error::KeyLen(n)),
        }
    }
}
```

Downstream takes `&Key` and never re-checks. Private fields plus a checked constructor keep the invariant true across every later refactor.

- Sum type over parallel flags: `enum Load { Idle, Running { since: u64 }, Failed(Cause) }` beats `bool` + `Option<u64>` + `Option<Cause>`, which admits combinations that mean nothing.
- No `_ =>` arm on an enum you own — adding a variant must break every match. Reserve `_` for foreign `#[non_exhaustive]` enums and numeric ranges.
- `match` when the compiler should force coverage; `if let` when one variant is genuinely the whole concern.
- Distinct roles get distinct types even at identical representation. Separate `EncryptNonce` and `DecryptNonce` make nonce reuse a compile error across the whole codebase.
- Single-use is enforced by taking the value **by move**, not by reference. A moved argument cannot be passed twice.

## Typestate

Encode legal call order into the type when order matters:

```rust
pub struct Frame<S> { id: FrameId, state: PhantomData<fn() -> S> }
pub struct Draft;
pub struct Sealed;

impl Frame<Draft>  { pub fn seal(self) -> Frame<Sealed> { Frame { id: self.id, state: PhantomData } } }
impl Frame<Sealed> { pub fn publish(&self) -> Result<(), Error> { Ok(()) } }
```

`publish` on a draft fails to compile. No runtime state field, no branch, no test for that branch.

The marker is `PhantomData<fn() -> S>`, never a bare `PhantomData<S>`, which would inherit `S`'s auto traits and drag `S` into drop check. That choice also makes the wrapper unconditionally `Send + Sync` regardless of `S`, which is correct **because** the type never owns an `S` — `har-unsafe` owns the full marker table and when a different one is needed.

Typestate is for a small finite set of mandatory transitions. Optional configuration is a builder, and validation that can only fail at runtime belongs in `build` (`har-api`).

## Integers

- Explicit widths everywhere. `usize`/`isize` only for lengths and indices.
- Debug builds panic on overflow, release builds wrap. Neither is correct for a value derived from input — pick the operation:
  - `checked_*` when overflow is an error: `a.checked_add(b).ok_or(Error::Overflow)?`
  - `saturating_*` when clamping is the spec
  - `wrapping_*` only when modular arithmetic *is* the spec (crypto, hashes, ring buffers)
  - `strict_*` **never**, on a reachable path: it panics on overflow in every profile, which is a panic you chose
  - where the wide result is what you actually want, `carrying_add` / `borrowing_sub` / `carrying_mul` and `checked_signed_diff` give it to you without an overflow to handle at all
- `as` truncates and flips sign silently. Narrow with `u32::try_from(x)?`, widen with `From`/`Into`. Never `as` on a value that came from outside the process.
- Overflow is defined in Rust (two's complement wrap), not UB — a wrong-answer bug, not corruption. Still a bug.
- It stops being only a wrong answer the moment the wrapped value feeds a length, an index, or an offset: there it becomes a soundness problem.
- `overflow-checks = true` in the profile that actually ships buys a loud failure for a small cost — name that profile rather than assuming it is `release`, since a project with its own `dist` or `production` profile inherits from `release` and can silently not have it.
- **Read that knob together with the panic strategy** (`har-layout`): with `panic = "abort"`, a surprise overflow is a process abort rather than a recoverable error. Under a zero-panic budget that makes the explicit `checked_*` the control and `overflow-checks` the backstop that proves you did not miss one — not an alternative to it.
- Atomics are exempt from `overflow-checks` — `fetch_add` wraps in every profile (`har-concurrent`).
- In a `const` context, overflow and out-of-bounds are **compile errors** rather than panics, so a const-evaluated expression and its runtime twin fail differently (`har-target`).

## API shape

- Parameters take `&[T]`, `&str`, `impl IntoIterator` — not `&Vec<T>`, `&String`. One function then serves arrays, vecs, and slices.
- Few public items, deep behavior behind them. The public surface is the misuse and stability budget; internals are free.
- `TryFrom`/`TryInto` for fallible conversion, `From`/`Into` for infallible. No bare cast at a boundary.
- `#[non_exhaustive]` on public error enums so a new variant is not a breaking change — from version one, since adding it later is itself breaking (`har-api`).
- Crate root posture: `#![forbid(unsafe_code)]` (`har-supply` owns the workspace form). `#![cfg_attr(not(feature = "std"), no_std)]` when the crate must run without an operating system — which is a separate decision from whether it allocates (`har-target`).
