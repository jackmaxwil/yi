---
name: har
description: High-assurance Rust patterns — newtypes, typestate, exhaustive enums, Result discipline, no panics on reachable paths, checked arithmetic, safe indexing. Load when writing or reviewing Rust that must not crash.
---

# High-assurance Rust: core patterns

## Panic budget

A panic on a reachable path is a bug. Every source has a non-panicking form:

| Panics | Use |
| --- | --- |
| `v[i]`, `v[a..b]` | `v.get(i)`, `v.get(a..b)` |
| `a + b`, `a - b`, `a * b` | `checked_*`, `saturating_*`, `wrapping_*` |
| `a / b`, `a % b` | `checked_div`, or type the divisor `NonZeroU32` |
| `.unwrap()`, `.expect()` | `?`, `.ok_or(Error::Missing)?`, `unwrap_or_default()` |
| `panic!`, `unreachable!`, `todo!` | return `Err` |
| `dst.copy_from_slice(src)` | length check first, or `dst.iter_mut().zip(src)` |
| `RefCell::borrow_mut` | `try_borrow_mut()?`, or restructure to `&mut` |
| `Vec::remove`, `Vec::insert`, `split_at` | `get`-guarded index, `split_at_checked` |
| unbounded recursion | iterate with an explicit stack |

`unwrap`/`expect` are allowed in tests, benches, and build scripts. Nowhere else.

`assert!` in a constructor kills the caller's process over the caller's mistake. Return `Result`. Keep `assert!` only for invariants no caller can influence.

## Result discipline

- Fallible public fn returns `Result<T, CrateError>`. Propagate with `?`.
- `let _ = fallible()` drops an error. Handle it or propagate it.
- One error variant per distinct caller reaction. Finer is noise; coarser strips the caller's ability to respond.
- Variants carry the offending value and the bound, not a rendered string:

```rust
enum Error { KeyTooShort { len: usize, min: usize }, KeyTooLong { len: usize, max: usize } }
```

- `Err(())` is fine inside a crate, never across a public API.
- `Option` means absence is normal. `Result` means the operation failed. Never encode failure as `None`.
- An infallible signature wrapping a fallible operation is the defect: `fn load() -> Config` that aborts on failure gives the caller nothing to handle.
- `#[must_use]` on functions returning `Option` or a value that is pointless unless inspected. `Result` already carries it.

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

The marker is `PhantomData<fn() -> S>`, never a bare `PhantomData<S>`, which would inherit `S`'s auto traits and drag `S` into drop check.

## Integers

- Explicit widths everywhere. `usize`/`isize` only for lengths and indices.
- Debug builds panic on overflow, release builds wrap. Neither is correct for a value derived from input — pick the operation:
  - `checked_*` when overflow is an error: `a.checked_add(b).ok_or(Error::Overflow)?`
  - `saturating_*` when clamping is the spec
  - `wrapping_*` only when modular arithmetic *is* the spec (crypto, hashes, ring buffers)
- `as` truncates and flips sign silently. Narrow with `u32::try_from(x)?`, widen with `From`/`Into`. Never `as` on a value that came from outside the process.
- Overflow is defined in Rust (two's complement wrap), not UB — a wrong-answer bug, not corruption. Still a bug.
- It stops being only a wrong answer the moment the wrapped value feeds a length, an index, or an offset: there it becomes a soundness problem.
- `overflow-checks = true` under `[profile.release]` buys a loud failure for a small cost.

## API shape

- Parameters take `&[T]`, `&str`, `impl IntoIterator` — not `&Vec<T>`, `&String`. One function then serves arrays, vecs, and slices.
- Few public items, deep behavior behind them. The public surface is the misuse and stability budget; internals are free.
- `TryFrom`/`TryInto` for fallible conversion, `From`/`Into` for infallible. No bare cast at a boundary.
- `#[non_exhaustive]` on public error enums so a new variant is not a breaking change.
- Crate root posture: `#![forbid(unsafe_code)]`, and `#![cfg_attr(not(test), no_std)]` when the crate is genuinely allocation-free.
