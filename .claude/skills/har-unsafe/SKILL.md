---
name: har-unsafe
description: Unsafe Rust semantics and audit — the UB list, validity vs safety invariants, SAFETY comment discipline, variance, drop check, PhantomData, borrow splitting, panic and leak safety, Send/Sync leakage, FFI and catch_unwind. Load when auditing unsafe you did not write, writing a justified unsafe module, or debugging a lifetime, variance, or Send error.
---

# Unsafe Rust: semantics and audit

A `forbid(unsafe_code)` crate still needs this. Variance, drop check, panic safety, leak amplification and Send/Sync leakage are safe-code rules — enforced by the compiler or exploited by a caller — and every FFI-shaped dependency (rusqlite, GPUI/Metal) puts unsafe in your call graph whether or not you wrote it. Start at **Consequences in safe code**; the rest is for reading someone else's.

## What unsafe unlocks

Exactly five permissions: deref a raw pointer; call an `unsafe` fn; implement an `unsafe` trait; access or modify a mutable `static`; access a `union` field. A block doing none of these is dead — delete it. `unsafe` disables no checking: borrowck, drop check and type checking run unchanged.

## The UB list

1. Data race — two threads, one location, at least one write, unsynchronized.
2. Load or store through a dangling or misaligned pointer.
3. Place projection outside the allocation (field or index out of bounds).
4. Aliasing violation — mutating behind a live `&T` outside `UnsafeCell`, or touching a live `&mut T`'s pointee through anything else. `Box<T>` counts as `&'static mut T`.
5. Mutating immutable bytes — const-promoted temporaries, `static`/`const` initializers, data behind `&`.
6. Invoking an intrinsic with invalid arguments.
7. Executing code compiled for `target_feature`s the CPU lacks.
8. Calling a function with the wrong ABI, or unwinding out of a non-unwinding ABI frame.
9. Producing an invalid value, alone or as a field.
10. Incorrect inline `asm!`.
11. Violating runtime assumptions, e.g. `longjmp` past frames with destructors.

Not UB, still bugs: integer overflow (defined wrap — but UB the moment it feeds a length or an offset), race conditions, leaks, deadlocks, aborts, `Drop` never running.

## Validity invariant vs safety invariant

An invalid value is UB the instant it exists, read or not:

| Type | Valid iff |
| --- | --- |
| `bool` | byte is 0 or 1 |
| `char` | ≤ `char::MAX`, not a surrogate `0xD800..=0xDFFF` |
| `!` | never |
| `fn` pointer | non-null |
| `i*`/`u*`/`f*` | initialized |
| `str` | initialized UTF-8 |
| `enum` | valid discriminant, every field of that variant valid |
| struct/tuple/array | every field valid at its own type |
| `&T`/`&mut T`/`Box<T>` | aligned, non-null, non-dangling, pointee valid |
| wide pointer | metadata matches tail: real vtable, slice len with `size ≤ isize::MAX` |
| `NonNull`/`NonZero` | inside the niche |

Dangling = the pointed-to bytes do not all lie in one live allocation; ZST pointers never dangle. Alignment comes from the *pointer's* type: `(*p).f` with `p: *const S` needs `S`'s alignment even when `f: u8` — `&raw const`/`&raw mut` on a misaligned place is fine, `&`/`&mut` is not. **Validity** is what the compiler assumes, owned by the language. **Safety** is what a type promises safe code (`Vec`'s `cap` really is the allocation; `str` is UTF-8 and callers may rely on it), owned by the module. Unsafe code may break the safety invariant inside the module; safe code outside must never observe it broken. `har` owns making invalid states unrepresentable; this is the layer under it.

## Soundness, and why the module is the unit

Sound = no use of the crate's safe API, however adversarial, can reach UB. A latent unsound API is as severe as an observed crash. Soundness is not local to the block — flip `<` to `<=` in safe code six lines up and `get_unchecked` reads out of bounds:

```rust
fn index(idx: usize, arr: &[u8]) -> Option<u8> {
    if idx < arr.len() {
        // SAFETY: idx < arr.len() checked immediately above; arr is a live slice.
        unsafe { Some(*arr.get_unchecked(idx)) }
    } else { None }
}
```

Privacy is the enforcement mechanism, so the unit of unsafety is the **module**: a safe private fn that corrupts the invariant is a bug but not unsound, and a diff touching only safe code in a module containing `unsafe` is still an unsafe review.

## SAFETY discipline

- `// SAFETY:` directly above every `unsafe { }`, naming the invariant and where it was established. A restatement of the code ("we deref the pointer") passes the lint and proves nothing — worse than none.
- `/// # Safety` on every `unsafe fn`, stating what the caller must guarantee. `// SAFETY:` on every `unsafe impl`.
- One unsafe block per obligation; never wrap a function body.
- `unsafe fn` means "has preconditions", not "contains unsafe ops" — each op inside still needs its own block.
- A **safe** fn's SAFETY may cite only: type invariants of its arguments, checks it performed itself, values it constructed. Citing a caller promise means it must be an `unsafe fn`.
- Enforce mechanically: `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, `missing_safety_doc` = `"deny"` under `[lints.clippy]`; `unsafe_op_in_unsafe_fn` = `"deny"` under `[lints.rust]`.

## Consequences in safe code

### Variance

| Type | over `'a` | over `T` |
| --- | --- | --- |
| `&'a T` | covariant | covariant |
| `&'a mut T` | covariant | **invariant** |
| `Box<T>`, `Vec<T>`, `[T]`, `Option<T>` | — | covariant |
| `Cell<T>`, `RefCell<T>`, `UnsafeCell<T>`, `Mutex<T>` | — | **invariant** |
| `*const T` / `*mut T` | — | covariant / **invariant** |
| `fn(T) -> U` | — | **contravariant** in `T`, covariant in `U` |

`&mut T` is invariant because it is a write channel: covariance would let you store a `&'short str` into a `&mut &'static str` and read it after the source dies. Diagnosis: "expected `Foo<'static>`, found `Foo<'a>`" with no visible cause is almost always an invariant position — an interior-mutability cell, a `&mut` field, a `PhantomData<*mut T>`. Variance is inferred structurally, so adding one `Cell<T>` field makes the struct invariant in `T` for every downstream user. That is an API break.

### Drop check and PhantomData

A type with a `Drop` impl requires its generic parameters to **strictly outlive** it; data merely stored need only outlive it. Adding `Drop` to an existing generic type breaks callers that compiled before — a breaking change, not a refactor. Restructure instead (split the struct, `Option::take`, drop explicitly, reorder fields); never reach for nightly `#[may_dangle]`.

| Marker | `'a` | `T` | Auto traits | Drop check |
| --- | --- | --- | --- | --- |
| `PhantomData<T>` | — | covariant | inherited from `T` | owns `T`; may not dangle |
| `PhantomData<&'a T>` | covariant | covariant | `Send + Sync` if `T: Sync` | may dangle |
| `PhantomData<&'a mut T>` | covariant | invariant | inherited | may dangle |
| `PhantomData<*const T>` | — | covariant | `!Send + !Sync` | may dangle |
| `PhantomData<*mut T>` | — | invariant | `!Send + !Sync` | may dangle |
| `PhantomData<fn(T)>` | — | contravariant | `Send + Sync` | may dangle |
| `PhantomData<fn() -> T>` | — | covariant | `Send + Sync` | may dangle |
| `PhantomData<fn(T) -> T>` | — | invariant | `Send + Sync` | may dangle |
| `PhantomData<Cell<&'a ()>>` | invariant | — | `Send + !Sync` | may dangle |

For a marker carrying no data — including the typestate parameter in `har` — use `PhantomData<fn() -> S>`, never `PhantomData<S>`. `PhantomData<S>` claims ownership of `S`, inherits its auto traits and drags it into drop check, so a `!Send` or `Drop`-bearing state type silently infects the whole struct; `fn() -> S` is covariant, unconditionally `Send + Sync`, and dropck-free. `PhantomPinned` removes `Unpin`; `PhantomData<(*mut u8, PhantomPinned)>` is the standard opaque-FFI marker (`!Send`, `!Sync`, `!Unpin`, invariant). If an invariant depends on a lifetime appearing in no field, `PhantomData` must reintroduce it.

### Borrow splitting without unsafe

Borrowck understands struct fields as disjoint and container indices not at all. Every apparent need for `unsafe` here has a safe tool:

| Need | Safe tool |
| --- | --- |
| two disjoint slice ranges | `split_at_mut`, `split_first_mut`, `split_last_mut` |
| n disjoint chunks | `chunks_mut`, `chunks_exact_mut`, `rchunks_mut` |
| two arbitrary indices | `slice::get_disjoint_mut([i, j])` |
| every element mutably | `iter_mut`, `iter_mut().enumerate()` |
| several map entries | `HashMap::get_disjoint_mut`, or `remove` + reinsert |
| two fields into one closure | destructure first: `let Foo { a, b } = &mut foo;` |
| mutation from a callback | pass ids, not references; re-look-up |
| take a field temporarily | `mem::take`, `mem::replace`, `Option::take` |
| self-referential graph | index arena (`Vec<Node>` + `NodeId`) |
| disjoint `&mut` across threads | `std::thread::scope` — no `'static`, no `Arc` |
`iter_mut` itself needs no unsafe because it consumes the remaining slice each step: `mem::take` the slice, `split_at_mut(1)`, store the tail back.

### Panic safety of your own types

Unwinding runs every destructor, so any `&mut self` method is observable mid-flight — through a `Drop` impl, through `Arc`/`Mutex` poisoning, or after a `catch_unwind`. Every user-supplied impl and closure can panic (`Clone`, `Drop`, `Ord`, `Hash`, `Display`, `Iterator::next`, `Extend`); assume it does. `har` owns not panicking; this is surviving someone else's panic. Rule: **never leave `self` violating its own documented invariant across a call you did not write.** Compute into locals, commit with one assignment or swap; bump the length counter after the element is written, never before. Where a value must be removed temporarily, hold it in a guard whose `Drop` restores a valid state, or `Option::take` and write back on both the success and the unwind path. Decide what a poisoned lock means rather than `.lock().unwrap()`. `panic = "abort"` makes all of this moot and is unavailable if anything relies on `catch_unwind` (test harnesses do); `catch_unwind` catches unwinds only, and `AssertUnwindSafe` is a claim, not a check.

### Leaks are safe; destructors are not guaranteed

`mem::forget`, `Box::leak`, an `Rc` cycle, and a panic in a destructor all skip `Drop`. Therefore **no invariant may depend on a destructor running.** Anything shaped like "this guard's `Drop` unlocks/joins/restores, therefore the borrow is sound" is unsound — that is why `thread::scoped` was removed and `thread::scope` replaced it. The safe fix, from `Vec::drain`: put the collection into a valid, empty state *before* handing out the proxy, so forgetting it leaks instead of corrupting. Review flag: any `*Guard`, `*Handle`, `Drain*`, `Scope*` in a dependency — ask what breaks if it is forgotten.

### Send and Sync leak

`Send` = movable to another thread. `Sync` = `&T: Send`. Both are derived structurally, so **a private field change silently alters your public API**: an `Rc`, `RefCell` or raw pointer three structs deep makes a top-level public type `!Send` and breaks every downstream `thread::spawn`. Sources to recognize: `Rc`/`Weak`, raw pointers, `Cell`/`RefCell`/`UnsafeCell` (`!Sync`), `MutexGuard` (`!Send`), GPUI/Metal handles, `rusqlite::Connection`. Defend it mechanically on every type that crosses a thread:

```rust
const _: fn() = || {
    fn assert<T: Send + Sync>() {}
    assert::<Frame>();
};
```

`unsafe impl Send`/`Sync` in a dependency is the ecosystem's highest-risk pattern: it replaces the compiler's structural reasoning with a human claim, and needs a justification naming the synchronization. Unsafe code may never assume safe code is race-free — a benign TOCTOU becomes UB the moment an `unsafe` block consumes the raced value.

## Raw pointers, aliasing, transmute, uninit

- `&mut T` promises the optimizer exclusivity for its whole live range; `&T` promises immutability except inside `UnsafeCell`, the only legal path from `&T` to a write. The rules attach at *creation*, not use — where you cannot promise them stay in raw pointers and use `&raw const`/`&raw mut`. Any route from `&` to `&mut`, transmute included, is UB.
- Derive pointers from the pointer owning the allocation: `ptr.add(n)` from the base is legal, rebuilding an address from a `usize` is not. `transmute` between `repr(Rust)` types has no layout guarantee — only `repr(C)`, `repr(transparent)`, `repr(int)` do. Transmuting to a reference with no named lifetime yields an unbounded lifetime that becomes `'static`; bind it. `transmute_copy` skips the size check. Preference order instead: `From`/`TryFrom`, `bytemuck`/`zerocopy`, `f32::to_bits`, `as`, `ptr::cast` + read, `union`. Pointer casts and unions obey the same rules, they just skip the lint.
- `MaybeUninit<T>` is the only legal container for uninit bytes. `ptr::write(p, v)`, never `*p = v` (which drops the garbage). Never form `&`/`&mut` to an uninit field — `&raw mut (*u.as_mut_ptr()).field`. `assume_init` asserts every byte is initialized *and* valid on every path. `Option<MaybeUninit<T>>` loses the niche.

## FFI

- Declare in `unsafe extern "C" { ... }`. Argument types are unchecked; a wrong signature is UB #8 and no tool will catch it. `libc` types (`c_int`, `size_t`) over Rust primitives.
- `#[repr(C)]` on every struct crossing the boundary, `#[repr(transparent)]` for an ABI-matching newtype. Never an empty enum for an opaque type — use `#[repr(C)] struct Opaque { _data: (), _marker: PhantomData<(*mut u8, PhantomPinned)> }`.
- `Option<extern "C" fn(...)>` for nullable callbacks: guaranteed null-pointer-optimized, and a null `fn` pointer is an invalid value.
- Validate alignment, null and provenance at the boundary, before the raw pointer becomes a reference. Afterwards is too late.
- Ownership is one-sided. Rust's `Box`/`Vec` use Rust's allocator: C `free()` on them is UB, `Box::from_raw` on a `malloc` pointer is UB. Where C owns, wrap the raw pointer in a type whose `Drop` calls C's free fn.
- Bind every `CString` to a local — `CString::new(s)?.as_ptr()` in one expression dangles at the semicolon. `CStr::from_ptr` needs a live, non-null, NUL-terminated pointer and returns an unbounded lifetime. Both directions are fallible: Rust `str` may hold interior NULs, C strings may not be UTF-8.
- `extern "C"` is non-unwinding: a panic reaching it aborts, a foreign exception entering it is UB. Every exported fn that can panic wraps its body, and callbacks handed to C are entry points with the same obligation — state travels as a `*mut T` userdata pointer, never a closure.

```rust
#[unsafe(no_mangle)]
pub extern "C" fn af_render(h: *mut Ctx) -> i32 {
    std::panic::catch_unwind(|| { 0 }).unwrap_or(-1)
}
```
Use `extern "C-unwind"` only when the other side genuinely propagates. miri cannot execute real FFI: for a C-backed dependency, correctness is review plus the C side's sanitizers.

## Auditing an unsafe block

1. Which of the five permissions does it use? None → delete it.
2. Does the `// SAFETY:` name the invariant and where it was established, or just restate the code?
3. For each unsafe operation, point at the line establishing its precondition. Established here, or assumed from the caller? Safe fn + caller assumption = unsound; it must be `unsafe fn`.
4. Which fields hold the invariant? All private, no public `&mut` escape, no `DerefMut` to the representation?
5. Read every safe fn in the same module — they can break it.
6. Can user code run while the invariant is broken (`Clone`, `Ord`, `Drop`, a closure, an iterator)? Does a panic there leave a valid state?
7. `mem::forget` every guard it hands out, mentally, and re-check.
8. Length/index/offset arithmetic: does it overflow before the bounds check? `isize::MAX` byte cap on allocations.
9. Generic `T`: holds for ZSTs, for a panicking `Drop`, for `!Send`?
10. Any `&`/`&mut` created that overlap or come from unclear provenance? Grep `from_raw_parts`, `transmute`, `&*ptr`.
11. Uninit: every byte written before `assume_init`? Reachable from another thread? Then the whole structure needs synchronization, not the block.
12. Run miri over the covering tests (`har-verify` rung 6). It detects exactly UB classes 1, 2, 4 and 9 — the right rung the moment an unsafe dependency sits in the hot path.

## Auditing an unsafe dependency

Reachability first: `unsafe` you never call is low priority (`har-supply` owns finding it). Then `rg -n 'unsafe' --type rust` in the vendored source, and read highest risk first:

| Pattern | Why |
| --- | --- |
| `unsafe impl Send`/`Sync` | overrides the whole thread-safety model on a human claim |
| `transmute` on `repr(Rust)` types | no layout guarantee; breaks on a compiler upgrade |
| `from_raw_parts` / `set_len` | length/capacity invariant, uninit exposure |
| `get_unchecked` | one off-by-one in *safe* code away from OOB |
| a `Guard` whose `Drop` is load-bearing | defeated by `mem::forget` |
| `static mut` | aliasing plus data race |
| `extern "C"` export with no `catch_unwind` | abort or UB on any panic |
| `&` → `&mut` by any route | always UB |

Read the module, not the block, and attack the safe API with a panicking `Clone`, a nonsense `Ord`, a `mem::forget`, two threads. Check RustSec history — past soundness advisories predict future ones. Record the verdict with the version audited, in `deny.toml` or the commit message; it expires at the next version bump. For Afterlife, GPUI/Metal and `rusqlite` are audit-by-thread-affinity problems rather than audit-by-pointer ones: main-thread affinity, `Connection` affinity, statement lifetimes, objc lifetime rules.
