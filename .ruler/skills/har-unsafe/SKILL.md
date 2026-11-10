---
name: har-unsafe
description: Unsafe Rust semantics and audit — the UB list, validity vs safety invariants, SAFETY comment discipline, provenance and strict-provenance APIs, aliasing models, variance, drop check, PhantomData, UnsafeCell, Pin, borrow splitting, panic and leak safety, Send/Sync leakage, repr and FFI, catch_unwind. Load when auditing unsafe you did not write, writing a justified unsafe module, or debugging a lifetime, variance, or Send error.
---

# Unsafe Rust: semantics and audit

A `forbid(unsafe_code)` crate still needs this. Variance, drop check, panic safety, leak amplification and Send/Sync leakage are safe-code rules — enforced by the compiler or exploited by a caller — and every FFI-shaped dependency (a database driver, a GPU or windowing binding, a codec) puts unsafe in your call graph whether or not you wrote it. Start at **Consequences in safe code**; the rest is for reading someone else's.

## What unsafe unlocks

An `unsafe` **block** permits five operation kinds: deref a raw pointer; call an `unsafe` fn; implement an `unsafe` trait; access or modify a mutable `static`; access a `union` field. A block doing none of these is dead — delete it. `unsafe` disables no checking: borrowck, drop check and type checking run unchanged.

Those five are not the whole of unsafe Rust. The Reference also treats as unsafe: calling a safe `#[target_feature]` function from a caller that lacks the feature, **declaring** an `extern` block, and applying an `unsafe` attribute. These are declarations and call preconditions, not operations a block makes safe — audit them separately.

| Syntax | Stable from | Required from |
| --- | --- | --- |
| `&raw const` / `&raw mut` | 1.82 | — (replaces `addr_of!`/`addr_of_mut!`) |
| `unsafe extern "C" { .. }` | 1.82 | edition 2024 (1.85) |
| `#[unsafe(no_mangle)]`, `#[unsafe(export_name)]`, `#[unsafe(link_section)]` | 1.82 | edition 2024 (1.85) |
| `slice::get_disjoint_mut`, `HashMap::get_disjoint_mut` | 1.86 | — |
| `ptr::addr`, `with_addr`, `map_addr` (strict provenance) | 1.84 | — |
| `<[T]>::as_chunks`, `as_chunks_mut`, `as_rchunks` | 1.88 | — (compile-time-checked indexing inside a chunk) |
| `MaybeUninit` slice APIs: `assume_init_ref`/`_mut`/`_drop`, `write_copy_of_slice`, `write_clone_of_slice` | 1.93 | — |
| `<[T]>::as_array`, `as_mut_array`, and the raw-pointer forms | 1.93 | — |
| `String::into_raw_parts`, `Vec::into_raw_parts` | 1.93 | — |
| `MaybeUninit<[T; N]>` ↔ `[MaybeUninit<T>; N]` conversions | 1.95 | — |
| `<*const T>::as_ref_unchecked`, `<*mut T>::as_mut_unchecked` | 1.95 | — |
| `Layout::dangling_ptr`, `repeat`, `repeat_packed`, `extend_packed` | 1.95 | — |
| `Vec::into_parts`/`from_parts`, `Box::into_non_null`/`from_non_null` | 1.99 | — |
| `size_of_val_raw`, `align_of_val_raw`, `Layout::for_value_raw` | 1.99 | — (size and align of a `?Sized` pointee without a reference) |

Two lints change posture at edition 2024: `unsafe_op_in_unsafe_fn` **warns** by default — unsafe operations inside an `unsafe fn` still compile, but each one wants its own block and its own SAFETY note — while `static_mut_refs` is **deny** by default.

## The UB list

1. Data race — two threads, one location, at least one write, unsynchronized.
2. Load or store through a dangling or misaligned pointer.
3. Place projection outside the allocation (field or index out of bounds).
4. Aliasing violation — mutating behind a live `&T` outside `UnsafeCell`, or touching a live `&mut T`'s pointee through anything else. `Box<T>` counts as `&'static mut T`.
5. Mutating immutable bytes — const-promoted temporaries, `static`/`const` initializers, data behind `&`. A `const` item holding a mutable reference to a `static` has been accepted since 1.93 and the release notes describe it exactly as "*very* unsafe, but not *always* UB" — which is to say the compiler stopped refusing the shape and did not make it sound. `const_item_interior_mutations` (1.93) warns on the calls that mutate through one.
6. Invoking an intrinsic with invalid arguments — including the `unchecked_*` arithmetic family (`unchecked_add`, and since 1.93 `unchecked_neg`, `unchecked_shl`, `unchecked_shr`), where overflow or an out-of-range shift is UB rather than a panic. They are `unsafe fn`s and the precondition is yours to prove; `checked_*` is the safe form (`har`).
7. Executing code compiled for `target_feature`s the CPU lacks.
8. Calling a function with the wrong ABI, or unwinding out of a non-unwinding ABI frame.
9. Producing an invalid value, alone or as a field.
10. Incorrect inline `asm!`.
11. Violating runtime assumptions, e.g. `longjmp` past frames with destructors.

Not UB, still bugs: integer overflow (defined wrap — but UB the moment it feeds a length or an offset), race conditions, leaks, deadlocks, aborts, `Drop` never running.

## Validity invariant vs safety invariant

An invalid value is UB the instant it **exists**, read or not — producing one is the violation:

| Type | Valid iff |
| --- | --- |
| every scalar, raw pointers included | initialized (no uninit bytes) |
| `bool` | byte is 0 or 1 |
| `char` | ≤ `char::MAX`, not a surrogate `0xD800..=0xDFFF` |
| `!` | never — there is no valid value |
| `fn` pointer | non-null |
| `str` | initialized UTF-8 |
| `enum` | valid discriminant, every field of that variant valid |
| struct/tuple/array/union | every field valid at its own type |
| `&T`/`&mut T`/`Box<T>` | aligned, non-null, non-dangling, pointee valid, `size_of_val ≤ isize::MAX` |
| wide **reference** or `Box` | metadata matches the tail: real vtable; slice len with total size ≤ `isize::MAX` |
| wide **raw pointer** | metadata correct and initialized — the `isize::MAX` size bound is a *reference*/`Box` requirement, and `dyn` metadata validity for raw pointers is still unsettled |
| `NonNull`/`NonZero` | inside the niche |

Dangling = the pointed-to bytes do not all lie in one live allocation; ZST pointers never dangle. Alignment comes from the *pointer's* type: `(*p).f` with `p: *const S` needs `S`'s alignment even when `f: u8`.

**Validity** is what the compiler assumes, owned by the language. **Safety** is what a type promises safe code (`Vec`'s `cap` really is the allocation; `str` is UTF-8 and callers may rely on it), owned by the module. Unsafe code may break the safety invariant inside the module; safe code outside must never observe it broken. `har` owns making invalid states unrepresentable; this is the layer under it.

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
- `unsafe fn` means "has preconditions", not "contains unsafe ops" — each op inside still needs its own block, and edition 2024 warns when it does not.
- A **safe** fn's SAFETY may cite only: type invariants of its arguments, checks it performed itself, values it constructed. Citing a caller promise means it must be an `unsafe fn`.
- Enforce mechanically: `undocumented_unsafe_blocks`, `multiple_unsafe_ops_per_block`, `missing_safety_doc` = `"deny"` under `[lints.clippy]`; `unsafe_op_in_unsafe_fn` = `"deny"` and `static_mut_refs` = `"deny"` under `[lints.rust]`, before the edition forces them.
- **Several rules this file states in prose are now lints.** Turn them on rather than reviewing for them: `integer_to_ptr_transmutes` (1.91) is exactly the provenance rule below; `dangling_pointers_from_locals` (1.91); `function_casts_as_integer` (1.93); `const_item_interior_mutations` (1.93); `raw_borrows_via_references` (1.99, allow-by-default, worth enabling); and `c_void_returns` (1.98) at an FFI boundary. Four are already deny-by-default and will find you on a toolchain bump instead of in review: `never_type_fallback_flowing_into_unsafe` and `dependency_on_unit_never_type_fallback` (1.92), `uninhabited_static` (1.96, now reported in **dependencies** too), and `invalid_runtime_symbol_definitions` (1.98, with `suspicious_runtime_symbol_definitions` beside it).

## Aliasing models: what is actually settled

Rust has **no finished aliasing specification**. Two experimental models exist: Miri uses **Stacked Borrows** by default, and `-Zmiri-tree-borrows` selects the more permissive, even more experimental **Tree Borrows**. Miri's own documentation says code Tree Borrows accepts today may still be UB under whatever model is eventually adopted.

So do not write rules against a model. Write them against the durable obligations, which no candidate model relaxes: `&mut T` is exclusive for its whole live range; `&T` is immutable except inside `UnsafeCell`; every access needs provenance, liveness, alignment and validity; and holding a raw pointer does not waive any of that. Run Miri under Stacked Borrows as the default gate and Tree Borrows as a second bug-finding configuration, and never report either one's silence as proof of soundness.

## Provenance

A pointer is not an address. It carries **provenance** — which allocation it may reach — alongside address, and bounds, liveness, alignment and aliasing are four further separate requirements. Casting a pointer to `usize` and back loses provenance, and the resulting access is UB even when the numeric address is identical.

Use the strict-provenance APIs (stable 1.84): `addr()` to read an address for comparison or hashing, `with_addr()` / `map_addr()` to compute a new pointer **from** a pointer that already carries the right provenance. Pointer tagging, alignment masking and free-list arithmetic all go through `map_addr`.

```rust
// SAFETY: tag occupies the low bit, which alignment of T >= 2 guarantees is free.
let tagged = ptr.map_addr(|a| a | 1);
let clean = tagged.map_addr(|a| a & !1);
```

`expose_provenance()` / `with_exposed_provenance()` and plain integer casts opt into the Exposed Provenance model, which the documentation says gives no definite guarantee about which provenance is selected and which tools can analyse it. Confine them to unavoidable platform and FFI cases, document why, and expect Miri coverage to be weaker there.

## Consequences in safe code

### Variance

| Type | over `'a` | over `T` |
| --- | --- | --- |
| `&'a T` | covariant | covariant |
| `&'a mut T` | covariant | **invariant** |
| `Box<T>`, `Vec<T>`, `[T]`, `Option<T>` | — | covariant |
| `Cell<T>`, `RefCell<T>`, `UnsafeCell<T>`, `Mutex<T>` | — | **invariant** |
| `*const T` / `*mut T` | — | covariant / **invariant** |
| `NonNull<T>` | — | **covariant** |
| `fn(T) -> U` | — | **contravariant** in `T`, covariant in `U` |

`&mut T` is invariant because it is a write channel: covariance would let you store a `&'short str` into a `&mut &'static str` and read it after the source dies. One narrow relaxation since 1.98: shortening the lifetime of a `&mut` is permitted when it is part of an **unsize coercion**, even in an invariant position — `Cell<&'long mut i32>` to `Cell<&'short mut dyn Send>` now compiles. Read the table as the rule and that as the one exception, rather than reading the table as absolute. Diagnosis: "expected `Foo<'static>`, found `Foo<'a>`" with no visible cause is almost always an invariant position — an interior-mutability cell, a `&mut` field, a `PhantomData<*mut T>`. Variance is inferred structurally, so adding one `Cell<T>` field makes the struct invariant in `T` for every downstream user. That is an API break.

**`NonNull<T>` is covariant, and that is a trap.** It looks like the owning-pointer type because it is non-null and carries the `Option` niche, but an owning or mutable container built on it inherits covariance and can be made unsound by shortening a lifetime and then writing. Add `PhantomData<Cell<T>>` or `PhantomData<&'a mut T>` where invariance is required, plus a separate ownership marker. `NonNull` is also `!Send` and `!Sync` by design — give the wrapper its own reasoned `unsafe impl`, never inherit silently.

### Drop check and PhantomData

A type with a `Drop` impl requires its generic parameters to **strictly outlive** it; data merely stored need only outlive it. Adding `Drop` to an existing generic type breaks callers that compiled before — a breaking change, not a refactor. Restructure instead (split the struct, `Option::take`, drop explicitly, reorder fields). The escape hatch, `#[may_dangle]`, is a nightly-only unsafe opt-out for a parameter the destructor provably never touches, and it is constrained by transitive drop glue — a type that owns a `T` cannot promise not to touch it while dropping. Do not reach for it in stable code.

| Marker | `'a` | `T` | Auto traits | Drop check |
| --- | --- | --- | --- | --- |
| `PhantomData<T>` | — | covariant | inherited from `T` | owns `T`; may not dangle |
| `PhantomData<&'a T>` | covariant | covariant | `Send + Sync` if `T: Sync` | may dangle |
| `PhantomData<&'a mut T>` | covariant | invariant | inherited | may dangle |
| `PhantomData<*const T>` | — | covariant | `!Send + !Sync` | may dangle |
| `PhantomData<*mut T>` | — | invariant | `!Send + !Sync` | may dangle |
| `PhantomData<fn(T)>` | — | contravariant | `Send + Sync` **regardless of `T`** | may dangle |
| `PhantomData<fn() -> T>` | — | covariant | `Send + Sync` **regardless of `T`** | may dangle |
| `PhantomData<fn(T) -> T>` | — | invariant | `Send + Sync` **regardless of `T`** | may dangle |
| `PhantomData<Cell<&'a ()>>` | invariant | — | `Send + !Sync` | may dangle |

Read the table in three columns at once — variance, auto traits, dropck — because the right marker depends on all three. For a marker carrying no data, including the typestate parameter in `har`, use `PhantomData<fn() -> S>`: covariant, dropck-free, and unconditionally `Send + Sync`. That last property is the point **and** the hazard — it deliberately does not inherit `S`'s auto traits, so it is correct only when the type genuinely neither owns nor can reach an `S`. Where ownership, drop order, or `S`'s thread-safety must propagate, use `PhantomData<S>` or the matching reference form instead.

`PhantomPinned` removes `Unpin`; `PhantomData<(*mut u8, PhantomPinned)>` is the standard opaque-FFI marker (`!Send`, `!Sync`, `!Unpin`, invariant). If an invariant depends on a lifetime appearing in no field, `PhantomData` must reintroduce it.

### UnsafeCell

`UnsafeCell<T>` is the **only** legal route from a `&` to a write, and that is all it is. It does not relax `&mut T`'s exclusivity, and it supplies **no synchronization** — two threads writing through one `UnsafeCell` is still a data race and still UB. Every abstraction built on it must state, in its SAFETY docs, what synchronizes the accesses and when a `&T` or `&mut T` to the payload may exist.

Layout note that bites transmuters: `UnsafeCell<T>` has `T`'s layout but **suppresses niches**, so `Option<UnsafeCell<T>>` is not `Option<T>`'s size and any size or representation reasoning that assumes it is, is unsound.

Since 1.99 the language guarantees that an `UnsafeCell`'s contents can be accessed **without going through `get()`** — a projection straight to the field is fine. That removes a dance, not an obligation: everything above about synchronization and about when a `&T` or `&mut T` to the payload may exist is unchanged.

### Pin, for the unsafe author

`Pin` is a library contract, not a compiler barrier — nothing stops a move at the machine level. A `!Unpin` value that has been pinned must stay at its address, and its storage must not be invalidated or reused, until its `drop` runs. That is the **drop guarantee**, and it is what self-referential futures rely on.

If you project through a pinned struct, decide per field whether it is *structurally pinned*, then honour it: a structurally pinned field may never be exposed through anything that permits a move (`&mut`, `mem::replace`, `Option::take`), the type may not be `repr(packed)`, and `Drop` must not move out of one. `Pin::new_unchecked` is an `unsafe fn` because it asserts the caller will keep that promise for the value's whole life. Add `PhantomPinned` to anything address-sensitive so it does not silently become `Unpin`. (`har-async` owns the everyday `pin!`/`Box::pin` usage.)

### Borrow splitting without unsafe

Borrowck understands struct fields as disjoint and container indices not at all. Every apparent need for `unsafe` here has a safe tool:

| Need | Safe tool |
| --- | --- |
| two disjoint slice ranges | `split_at_mut`, `split_first_mut`, `split_last_mut` |
| n disjoint chunks | `chunks_mut`, `chunks_exact_mut`, `rchunks_mut` |
| two arbitrary indices | `slice::get_disjoint_mut([i, j])` — MSRV 1.86, returns `Result` on overlap or out-of-bounds |
| several map entries | `HashMap::get_disjoint_mut` — MSRV 1.86; returns `[Option<_>; N]`, **panics** on duplicate keys, and its duplicate check is O(N²) |
| every element mutably | `iter_mut`, `iter_mut().enumerate()` |
| two fields into one closure | destructure first: `let Foo { a, b } = &mut foo;` |
| mutation from a callback | pass ids, not references; re-look-up |
| take a field temporarily | `mem::take`, `mem::replace`, `Option::take` |
| self-referential graph | index arena (`Vec<Node>` + `NodeId`) |
| disjoint `&mut` across threads | `std::thread::scope` — no `'static`, no `Arc` |

Never the `*_unchecked` variants unless non-overlap is *proved*: overlap is UB even if you discard the outputs. `iter_mut` itself needs no unsafe because it consumes the remaining slice each step: `mem::take` the slice, `split_at_mut(1)`, store the tail back.

### Panic safety of your own types

Unwinding runs every destructor, so any `&mut self` method is observable mid-flight — through a `Drop` impl, through `Arc`/`Mutex` poisoning, or after a `catch_unwind`. Every user-supplied impl and closure can panic (`Clone`, `Drop`, `Ord`, `Hash`, `Display`, `Iterator::next`, `Extend`); assume it does. `har` owns not panicking; this is surviving someone else's panic. Rule: **never leave `self` violating its own documented invariant across a call you did not write.** Compute into locals, commit with one assignment or swap; bump the length counter after the element is written, never before. Where a value must be removed temporarily, hold it in a guard whose `Drop` restores a valid state, or `Option::take` and write back on both the success and the unwind path. Decide what a poisoned lock means rather than `.lock().unwrap()`. `panic = "abort"` makes all of this moot and is unavailable if anything relies on `catch_unwind` (test harnesses do); `catch_unwind` catches unwinds only, and `AssertUnwindSafe` is a claim, not a check.

### Leaks are safe; destructors are not guaranteed

`mem::forget`, `Box::leak`, an `Rc` cycle, and a panic in a destructor all skip `Drop`. Therefore **no invariant may depend on a destructor running.** Anything shaped like "this guard's `Drop` unlocks/joins/restores, therefore the borrow is sound" is unsound — that is why `thread::scoped` was removed and `thread::scope` replaced it. The safe fix, from `Vec::drain`: put the collection into a valid, empty state *before* handing out the proxy, so forgetting it leaks instead of corrupting. Review flag: any `*Guard`, `*Handle`, `Drain*`, `Scope*` in a dependency — ask what breaks if it is forgotten. (Pin's drop guarantee is the one place the language does demand a destructor run, and it is why `Pin::new_unchecked` is unsafe.)

### Send and Sync leak

`Send` = movable to another thread. `Sync` = `&T: Send`. Both are derived structurally, so **a private field change silently alters your public API**: an `Rc`, `RefCell` or raw pointer three structs deep makes a top-level public type `!Send` and breaks every downstream `thread::spawn`. Sources to recognize: `Rc`/`Weak`, raw pointers, `NonNull`, `Cell`/`RefCell`/`UnsafeCell` (`!Sync`), `MutexGuard` (`!Send`), thread-affine platform handles (GPU, windowing, objc), and connection handles that are affine to the thread that opened them. Defend it mechanically on every type that crosses a thread:

```rust
const _: fn() = || {
    fn assert<T: Send + Sync>() {}
    assert::<Frame>();
};
```

`unsafe impl Send`/`Sync` in a dependency is the ecosystem's highest-risk pattern: it replaces the compiler's structural reasoning with a human claim, and needs a justification naming the synchronization. Unsafe code may never assume safe code is race-free — a benign TOCTOU becomes UB the moment an `unsafe` block consumes the raced value.

### Mutable statics

Merely **forming** a reference to a `static mut` — shared or mutable, used or not — can violate aliasing on the spot, which is why `static_mut_refs` is deny-by-default in edition 2024. The fix is not `&raw mut` alone: that avoids creating the reference, and leaves every synchronization, reentrancy, interrupt and panic obligation exactly where it was. Prefer an atomic, a `Mutex`/`RwLock`, or `OnceLock`. If a raw pointer is genuinely unavoidable, take it with `&raw mut`, form a reference only at the point exclusivity is proved, and audit threads, signals, reentrancy and `Drop`.

## Raw pointers, aliasing, transmute, uninit

- `&mut T` promises the optimizer exclusivity for its whole live range; `&T` promises immutability except inside `UnsafeCell`. The rules attach at *creation*, not use — where you cannot promise them, stay in raw pointers and use `&raw const`/`&raw mut`. Any route from `&` to `&mut`, transmute included, is UB.
- **A raw borrow is address formation, nothing more.** `&raw const`/`&raw mut` legally names a misaligned, uninitialized or aliased place where `&`/`&mut` would already be UB — but the place projection must still be in-bounds, and each later read or write carries its own alignment, validity, provenance, liveness and aliasing obligation. A pointer taken with `&raw const` is not writable unless the storage is in an `UnsafeCell`; use `&raw mut` where mutation is intended. For a `repr(packed)` field: `&raw const field` + `read_unaligned`, or `&raw mut field` + `write_unaligned` — never a reference, never an ordinary deref.
- Derive pointers from the pointer owning the allocation: `ptr.add(n)` from the base is legal, rebuilding an address from a `usize` is not (see **Provenance**). `transmute` between `repr(Rust)` types has no layout guarantee — only `repr(C)`, `repr(transparent)`, `repr(int)` do. Transmuting to a reference with no named lifetime yields an unbounded lifetime that becomes `'static`; bind it. `transmute_copy` skips the size check. Preference order instead: `From`/`TryFrom`, `bytemuck`/`zerocopy`, `f32::to_bits`, `as`, `ptr::cast` + read, `union`. Pointer casts and unions obey the same rules, they just skip the lint.
- `MaybeUninit<T>` is the only legal container for uninit bytes. `ptr::write(p, v)`, never `*p = v` (which drops the garbage). Never form `&`/`&mut` to an uninit field — `&raw mut (*u.as_mut_ptr()).field`. `assume_init` asserts every byte is initialized *and* valid on every path. `Option<MaybeUninit<T>>` loses the niche.
- **Zero-sized types are a separate branch, always.** A suitably aligned pointer with no provenance may be used for a zero-sized access, and pointer arithmetic on a ZST is a no-op. An allocator must never receive a zero-size layout, so a `Vec`-like type must not deallocate the sentinel it never allocated. `NonNull::dangling()` is non-null and correctly aligned and proves **nothing** about liveness, provenance, or a non-empty slice — it is a sentinel, not permission to dereference. Branch on `size_of::<T>() == 0` in every allocate, deallocate and iterator-arithmetic path.

## FFI

### Layout

`#[repr(C)]` fixes a struct's or union's field order and padding using **each field's own layout** — it does not recursively make nested `repr(Rust)` types C-shaped, and it does not validate anything a foreign caller hands you. `#[repr(transparent)]` delegates ABI and layout to its single non-zero-sized field. A `repr(C)` enum *with fields* is a tagged union; a C-like enum needs an explicit `repr(int)` matching the C side. `repr(packed)` makes a reference to an under-aligned field UB — see the raw-borrow rule above.

So: use C ABI scalar and pointer types (`libc`'s `c_int`, `size_t`) rather than Rust primitives, mark every boundary type `repr(C)` or `repr(transparent)` deliberately, and **validate** tags, pointers and lengths before constructing a Rust reference or enum from them. Rust `bool`, references, and generic types are not FFI-safe just because the struct around them says `repr(C)`.

### Declarations and lifetimes

- Declare in `unsafe extern "C" { ... }`. Argument types are unchecked; a wrong signature is UB #8 and no tool will catch it.
- Never an empty enum for an opaque type — use `#[repr(C)] struct Opaque { _data: (), _marker: PhantomData<(*mut u8, PhantomPinned)> }`.
- `Option<extern "C" fn(...)>` for nullable callbacks: guaranteed null-pointer-optimized, and a null `fn` pointer is an invalid value.
- Validate alignment, null and provenance at the boundary, before the raw pointer becomes a reference. Afterwards is too late.
- Ownership is one-sided. Rust's `Box`/`Vec` use Rust's allocator: C `free()` on them is UB, `Box::from_raw` on a `malloc` pointer is UB. Where C owns, wrap the raw pointer in a type whose `Drop` calls C's free fn.
- Bind every `CString` to a local — `CString::new(s)?.as_ptr()` in one expression dangles at the semicolon. `CStr::from_ptr` needs a live, non-null, NUL-terminated pointer and returns an unbounded lifetime. Both directions are fallible: Rust `str` may hold interior NULs, C strings may not be UTF-8.

### Unwinding

Four things interact, and conflating them is how a boundary aborts in production:

| | `extern "C"` | `extern "C-unwind"` |
| --- | --- | --- |
| Rust panic reaching it | **UB** under `panic="unwind"` — catch it first | propagates, if the other side genuinely supports it end to end |
| Foreign exception entering it | **UB** | propagates |
| Under `panic="abort"` | panic aborts before reaching the boundary | same |

`catch_unwind` catches **Rust unwinds only**. Catching a foreign exception through it has unspecified behaviour — abort, or an opaque `Err` — so it is not a C++ exception firewall, and it catches nothing at all under `panic="abort"`. Whether destructors run at a non-unwinding boundary under `panic="unwind"` is likewise unspecified.

The rule: export C-facing functions as non-unwinding, convert every panic and error into an explicit status code at the boundary, and use `C-unwind` only where cross-language unwinding is deliberately designed and supported on both sides — never against `-fno-exceptions` code. Callbacks you hand to C are entry points with the same obligation; state travels as a `*mut T` userdata pointer, never a closure.

```rust
/// # Safety
/// `ctx` must be a pointer previously returned by `ctx_new` and not yet freed.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn render(ctx: *mut Ctx) -> i32 {
    let Some(ctx) = (unsafe { ctx.as_mut() }) else { return ERR_NULL };
    match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| ctx.render())) {
        Ok(Ok(())) => 0,
        Ok(Err(e)) => e.code(),
        Err(_) => ERR_PANIC,
    }
}
```

`AssertUnwindSafe` is needed because `&mut Ctx` is captured; it asserts that a panic mid-render leaves `Ctx` in a state the caller may still observe — which the **Panic safety** rules above are what make true. miri cannot execute real FFI: for a C-backed dependency, correctness is review plus the C side's sanitizers.

## Beyond C: C++ and Python

Both languages add a runtime with its own rules, so "it is just FFI" is where these boundaries go wrong. Use a generated bridge that checks signatures statically — `cxx`, `autocxx`, `pyo3` — rather than hand-declaring across the gap.

### C++

- **Types are opaque unless layout, alignment, move semantics, destruction, aliasing and lifetime are all jointly specified.** A C++ type with a non-trivial move constructor has no Rust equivalent: Rust moves are `memcpy`, C++ moves run code. That is why `cxx` hands you `UniquePtr<T>`/`SharedPtr<T>` and pinned references rather than a Rust value.
- Ownership transfers only through the forms the bridge supports — `Box<T>` one way, `UniquePtr`/`SharedPtr` the other. Never free across allocators (the C rule, unchanged).
- **Exception policy is part of the ABI contract.** An exception escaping a function `cxx` declared as non-throwing terminates the process, and a Rust panic crossing `extern "Rust"` aborts. Decide where exceptions are caught and translated into a `Result`, and write it down on both sides.
- `const T&` in C++ does not mean `&T` in Rust — C++ has no aliasing guarantee, so a `const&` can change under you. Model it as a raw pointer or a pinned reference unless the C++ side documents otherwise.

### Python

- A `Bound<'py, T>` or any borrowed Python reference is valid only for the attachment it came from. **Never store one in a struct that outlives the boundary** — that is the single most common `pyo3` unsoundness. `Py<T>` is the owned form, and it carries explicit reference-count, cycle and drop semantics you must think about.
- Acquire and propagate `Python<'py>` at the boundary only; do not thread it through the pure-Rust interior.
- **Release the GIL before anything that can block** — a Rust mutex, an `await`, a long native computation — or you have serialized the whole interpreter and invented a deadlock between the GIL and your own lock.
- Rust errors convert to `PyErr` at the boundary; a panic must not cross into the interpreter uncaught.
- Free-threaded builds change the GIL assumption entirely. State which build you support and test on it.

## Auditing an unsafe block

1. Which of the five permissions does it use? None → delete it. Any unsafe *declaration* or `#[target_feature]` call nearby gets its own review.
2. Does the `// SAFETY:` name the invariant and where it was established, or just restate the code?
3. For each unsafe operation, point at the line establishing its precondition. Established here, or assumed from the caller? Safe fn + caller assumption = unsound; it must be `unsafe fn`.
4. Which fields hold the invariant? All private, no public `&mut` escape, no `DerefMut` to the representation?
5. Read every safe fn in the same module — they can break it.
6. Can user code run while the invariant is broken (`Clone`, `Ord`, `Drop`, a closure, an iterator)? Does a panic there leave a valid state?
7. `mem::forget` every guard it hands out, mentally, and re-check.
8. Length/index/offset arithmetic: does it overflow before the bounds check? `isize::MAX` byte cap on allocations.
9. Generic `T`: holds for ZSTs, for a panicking `Drop`, for `!Send`, for a `!Unpin` self-referential value?
10. Any `&`/`&mut` created that overlap or come from unclear provenance? Grep `from_raw_parts`, `transmute`, `&*ptr`, `as *mut`, `with_exposed_provenance`.
11. Uninit: every byte written before `assume_init`? Reachable from another thread? Then the whole structure needs synchronization, not the block.
12. If it projects through a `Pin`: which fields are structurally pinned, and is the drop guarantee preserved on every path?
13. Run miri over the covering tests (`har-verify` rung 6), then again with `-Zmiri-tree-borrows`. It detects exactly UB classes 1, 2, 4 and 9, on the paths your tests execute — nothing more.

## Auditing an unsafe dependency

Reachability first: `unsafe` you never call is low priority (`har-supply` owns finding it). Then `rg -n 'unsafe' --type rust` in the vendored source, and read highest risk first:

| Pattern | Why |
| --- | --- |
| `unsafe impl Send`/`Sync` | overrides the whole thread-safety model on a human claim |
| `transmute` on `repr(Rust)` types | no layout guarantee; breaks on a compiler upgrade |
| `from_raw_parts` / `set_len` | length/capacity invariant, uninit exposure |
| `get_unchecked`, `*_unchecked` | one off-by-one in *safe* code away from OOB |
| a `Guard` whose `Drop` is load-bearing | defeated by `mem::forget` |
| `static mut` | aliasing plus data race; deny-by-default in 2024 for a reason |
| pointer↔integer casts, `with_exposed_provenance` | provenance laundering; weak tool coverage |
| `Pin::new_unchecked`, hand-written projections | drop guarantee and structural pinning |
| `extern "C"` export with no panic conversion | abort or UB on any panic |
| `&` → `&mut` by any route | always UB |

Read the module, not the block, and attack the safe API with a panicking `Clone`, a nonsense `Ord`, a `mem::forget`, two threads. Check RustSec history — past soundness advisories predict future ones. Record the verdict with the version audited, in `deny.toml` or the commit message; it expires at the next version bump.

Not every risky dependency is a pointer problem. A GPU, windowing, or objc binding and a single-connection database driver are **thread-affinity** audits instead: which handle belongs to which thread, how long a statement or view may outlive its parent, and what the platform's own lifetime rules are. The finding there is a `Send`/`Sync` claim, not a `transmute`.
