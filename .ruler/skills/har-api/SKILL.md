---
name: har-api
description: Rust API and ownership shape — generics vs dyn Trait, dyn compatibility and upcasting, sealed and extension traits, RPITIT and edition-2024 RPIT capture, standard traits to derive, must_use, From/Into and builders, Drop and RAII, lifetimes and the borrow-checker ladder, Rc/RefCell/Cow choices, iterator and combinator idioms, thiserror and error source chains, visibility, MSRV, semver. Load when designing a type, trait, or public interface.
---

# API and ownership shape

`har` decides what a type may **be** — validity, states, failure, arithmetic, panic freedom. This skill decides how it is **shaped, dispatched, borrowed, and handed out**. Where they touch (errors, conversions, `RefCell`), `har` owns the correctness rule and this file owns the ergonomics.

## Traits and dispatch

| Axis | Generic `<T: Trait>` | `dyn Trait` |
| --- | --- | --- |
| Dispatch | static, monomorphized | vtable, two derefs |
| Binary size / compile time | one copy per type, slower build | one copy, faster build |
| Multiple bounds | free (`T: Debug + Draw`) | needs an invented combined trait |
| Upcast to a supertrait | works | works since Rust 1.86 |
| Heterogeneous collection | impossible | the reason it exists |
| Type known only at runtime | no | yes |
| Constraints | none | no generic methods, no `-> Self`, no GATs, no `async fn` |

Default to generics. Switch to `dyn` for a heterogeneous collection or a type unknown until runtime. Switch for code size or build time **only after measuring**.

**Dyn compatibility** (the Reference's current name for what was called object safety): no generic methods, no method returning `Self` or a `Self`-containing type, no generic associated types, no `async fn` or RPITIT method. `where Self: Sized` exempts one method and keeps the rest of the trait dyn-compatible. Since **1.86** a `dyn Sub` coerces directly to `dyn Super` for a declared supertrait — through `&`, `Rc`, `Arc`, and raw pointers — so do not add an `as_super()` method for that alone. Below that MSRV the explicit upcast method is still required.

| `impl Trait` position | Means | Use when | Don't |
| --- | --- | --- | --- |
| Argument `fn f(x: impl Draw)` | anonymous type parameter; caller cannot turbofish | the bound appears once and is never named | the bound appears twice, the caller must pick the type, or an RPIT `use<..>` list must name the parameter |
| Return `fn f() -> impl Iterator<Item = u8>` | one hidden concrete type | returning a closure or an adapter chain | branches return different concrete types — that needs `Box<dyn Trait>` |

Swapping argument-position `impl Trait` for a named generic, or back, changes the caller's generic-argument interface. It is a semver-sensitive edit, not a refactor.

**RPIT capture is edition-dependent.** In edition 2024 a free function or inherent method returning `impl Trait` captures **all** in-scope lifetime, type and const parameters by default; before 2024 it captured only lifetimes named in the bounds. That changes what the caller may still borrow. Write `+ use<'a, T>` (stable since 1.82) for a deliberate capture set, and run the `impl_trait_overcaptures` lint when migrating an edition. Never reach for the old `Captures` trait or an `outlives` trick.

**`async fn` and RPITIT in a public trait are a permanent commitment.** Both are stable since 1.75 and both make the trait not dyn-compatible. Worse: the returned opaque future carries no `Send` bound, callers cannot add one, and you cannot relax or add one later without a major break. So decide before publishing:

| Need | Shape |
| --- | --- |
| Static dispatch, futures stay on one thread | native `async fn` in the trait |
| Static dispatch, futures must be `Send` | `#[trait_variant::make(Trait: Send)]`, or hand-write `-> impl Future<Output = T> + Send` |
| `dyn` dispatch | `#[async_trait]`, or a separate dyn-compatible trait returning `Pin<Box<dyn Future + Send>>` |

**Seal a trait, or document that it is open.** An open public trait can never gain a required method, and even a defaulted addition can break an implementor through method-name ambiguity. If outside implementations are not part of the contract, seal it from version one with a private supertrait and say so in the docs:

```rust
mod sealed { pub trait Sealed {} }
pub trait Frame: sealed::Sealed { fn id(&self) -> FrameId; }
```

**Extension traits** are the tool for methods you cannot make inherent — on a foreign type, or to split generic and consuming conveniences off a core trait you promised for `dyn`. Name them `*Ext`, keep them narrow, and re-export from a prelude only when blanket import is the intent.

**Generic associated types** (stable 1.65) express an output borrowing from `&mut self` — a lending iterator or a zero-copy parser — without allocation or self-reference. They cost dyn compatibility for the whole trait, and their `where Self: 'a` bounds must be designed in, never retrofitted. Offer a separate owned or erased interface when both are needed.

Few required methods, many provided ones: implementor cost is the scarce resource, `Iterator` is the shape to copy (one `next`, fifty provided). A provided method may carry its own bound so it appears only for qualifying types. Adding a defaulted method later is *possibly* breaking — name collisions and method resolution — not free.

Take `impl Fn(..)` or `F: Fn(..)`, never a bare `fn(..)` pointer — a pointer rejects every closure with captures. Accept the most general trait that works: `FnOnce` ⊃ `FnMut` ⊃ `Fn`. Marker traits with no methods encode promises the signature cannot.

## Standard traits

| Trait | Default | Exception |
| --- | --- | --- |
| `Debug` | derive always | secrets, keys, PII — hand-write a redacting impl, never silently omit |
| `Clone` | derive | not on an RAII / unique-resource type |
| `Copy` | derive when a bitwise copy is valid and the type is small | id newtypes, data-free enums; never anything large. Never `.clone()` a `Copy` type |
| `PartialEq` + `Eq` | derive together | `PartialEq` alone only for float-bearing types |
| `PartialOrd` + `Ord` | both or neither | derived order is lexicographic by field, so field order is API |
| `Hash` | derive when the type is a map key | if `Eq` is hand-written, `Hash` must be too: `x == y` ⇒ `hash(x) == hash(y)` |
| `Default` | derive when a meaningful zero exists | never invent a default that is invalid |
| `Display` | never derive; hand-write | only for end-user-facing types. `Debug` is for programmers. For a one-off rendering that does not deserve a type at all, `fmt::from_fn` (1.93) gives you a `Display` from a closure |

House line: `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]` on every id newtype, `#[derive(Debug, Clone, PartialEq)]` on every plain data struct.

`#[must_use = "…"]` applies to functions **and to types**, and the reason string is where the caller learns what they dropped. Put it on anything whose discard loses the only evidence that work did not happen: a `close`/`commit` result, a guard, a validated token, a linear protocol state. Leave it off a deliberately fire-and-forget handle. Adding it to a shipped API is a new warning downstream, and downstream may run `-D warnings` — so decide at introduction.

The compiler no longer nags about a `Result` that cannot fail: since 1.92 `unused_must_use` does not fire on `Result<(), !>` or `ControlFlow<!, ()>`, and since 1.97 a `Result<T, Uninhabited>` or `ControlFlow<Uninhabited, T>` is treated as `T` for the lint. So an infallible-by-type `Result` in a generic signature stays honest without forcing every caller to write `let _ =`.

Three hard don'ts: never implement `Deref` for a non-pointer type; never overload an operator across unrelated types; never implement half an operator set — `Add` + `Neg` obliges `Sub`, and `x - y == x + (-y)` must hold.

The `Deref` ban needs a replacement, or the newtype is unusable. Give the wrapper exactly the operations its contract needs — domain accessors, `as_inner()` where a borrow is part of the contract, `into_inner()` where ownership escape is — plus whichever standard traits genuinely hold. `AsRef` only when the conversion is cheap and the contract survives it. Private field throughout. That is the point of the newtype: hide the representation so it can change.

## Drop and RAII

Constructor acquires, destructor releases. `Drop::drop` takes `&mut self` and returns `()`, so **a destructor cannot report failure**: expose `fn close(self) -> Result<(), Error>` for fallible release — `#[must_use]`, per above — and let `drop` do the best-effort remainder. `x.drop()` does not compile — use `drop(x)` or an inner block. Guards release correctly through early returns and `?`. `Drop` is not guaranteed to run (leak, abort, `forget`), so it is never the only path for cleanup that must happen.

## Conversion and construction

Implement `From`, never `Into` — the blanket impl gives `Into` free. Bound generic parameters on `Into`, not `From`, so both direct and blanket impls match; the reflexive `impl From<T> for T` is why an `Into<T>` bound still accepts a `T`. User types join coercion only via `Deref`/`DerefMut` or concrete→`dyn Trait`. A newtype also defeats the orphan rule: wrap a foreign type to implement a foreign trait on it. (`har` owns `TryFrom` vs `From` and the ban on bare `as`.)

`AsRef` for cheap explicit reference conversion, `Borrow` when a function must accept owned or borrowed alike (`HashMap::get`), `ToOwned`/`Cow` when the borrowed form is usual and the owned form occasional.

| Builder | Setter | Wins | Loses |
| --- | --- | --- | --- |
| Consuming | `fn title(mut self, ..) -> Self` | fluent chain straight off `new` | conditional setters need reassignment; `build` callable once |
| Mutating | `fn title(&mut self, ..) -> &mut Self` | conditional setters, repeatable `build` | must name the builder; `build(&self)` needs `Clone` or manual reconstruction |

```rust
let mut builder = FrameBuilder::new(id);
builder.title("draft").gutter(true);
if pinned { builder.pin(); }
let frame = builder.build()?;
```

`build` returns `Result` — the builder is where deferred validation lands.

Builder and typestate are not alternatives; they answer different questions. A builder holds **optional configuration** and validates at `build`, including anything only checkable at runtime. Typestate (`har`) encodes a **small, finite, mandatory call order** that the compiler can check, and is wrong for validation that can only fail later. A builder with three `Option` fields that must all be set is a typestate wearing the wrong costume; a typestate parameter threaded through ten optional settings is a builder wearing the wrong one. Keep the state marker private unless the states are deliberately public API.

## Ownership and borrowing

| Situation | Take |
| --- | --- |
| Parameter, read-only | `&str` / `&[T]` / `impl IntoIterator` |
| Struct field, any doubt | **owned** `String` / `Vec<T>` — a lifetime param infects every holder, transitively |
| Struct field, provably shorter-lived than its source, measured hot path | `&'a T` |
| Usually borrowed, occasionally extended or mutated | `Cow<'a, T>` |
| Output borrowing from `&mut self`, no allocation | GAT on a trait, per above |
| Small immutable value the borrow checker is fighting over | clone it |

A lifetime is the span from creation to drop **or move**. Elision, in three rules: one input reference gives every output reference its lifetime; several input references with no output reference each get their own; a method taking `&self` gives outputs `self`'s lifetime. None of the three covers an opaque return — that is the RPIT capture rule above. `'_` names an elided lifetime without inventing one. `'static` comes from statics, promoted consts, and `Box::leak` — not from "lives a long time".

When the borrow checker rejects the code, climb in order and stop at the first rung that works:

| Try | Before reaching for |
| --- | --- |
| Extend a borrow with a `let` binding, or end one early with an inner block | anything |
| Split the chained expression into annotated `let` steps to find the failing operation | anything |
| Redesign for single ownership; store an **index**, not an interior reference | `Rc` |
| Clone the small immutable thing | `Rc` |
| `Rc<RefCell<T>>`, `Weak` for cycles, `Arc<Mutex<T>>` across threads | a self-referential struct |
| `ouroboros` | a hand-rolled self-reference |

`Rc<RefCell<T>>` is a design, not a defeat — but it turns a compile error into a runtime borrow panic, so `try_borrow_mut()?` per `har`. Nesting order is a decision: `Rc<RefCell<Vec<T>>>` shares a collection mutated as a whole; `Rc<Vec<RefCell<T>>>` shares a collection whose elements mutate independently. Zero-copy is not free — an owning `Vec<u8>` usually beats a `&'a [u8]` field. (`har-concurrent` owns lock discipline.)

## Data flow

Reach for a combinator first; `match` only when several arms need genuinely different control flow, `if let` when one arm is the whole concern.

| Want | `Option` | `Result` |
| --- | --- | --- |
| Transform the value / the failure | `map` | `map` / `map_err` |
| Chain a fallible step | `and_then` | `and_then` |
| Fallback value, or alternative | `unwrap_or_else`, `or_else` | same |
| Drop the error | — | `ok()` |
| Supply an error for absence | `ok_or_else` | — |
| Keep only if it passes a test | `filter` | — |
| Borrow the inside instead of moving | `as_ref`, `as_mut`, `as_deref` | `as_ref` |
| Propagate | `?` | `?` (converts via `From`) |
| `Vec<Result<T, E>>` → `Result<Vec<T>, E>` | — | `.collect::<Result<Vec<_>, _>>()?` |

| Loop construct | Adapter |
| --- | --- |
| `if cond { continue }` / `if cond { break }` | `.filter(..)` / `.take_while(..)` |
| counter with a bound | `.take(n)` |
| manual index | `.enumerate()` |
| accumulator `+=` | `.sum()` / `.fold(init, f)` |
| flag set in the loop | `.any(..)` / `.all(..)` — both short-circuit |
| find-first then `break` | `.find(..)` / `.position(..)` |
| push into two vecs by predicate | `.partition(..)` |
| nested loop over a nested collection | `.flat_map(..)` / `.flatten()` |
| body is fallible, stop at first `Err` | `.collect::<Result<Vec<_>, _>>()?` / `.try_fold(..)` |
| skipping `None`s | `.flatten()` |

Keep the explicit loop when the body is large or multi-purpose, when it returns from the enclosing function, or when a benchmark says so — closures normally optimize identically.

## Errors at the boundary

| | Library crate | Binary crate |
| --- | --- | --- |
| Error type | `enum` + `thiserror` derive | `anyhow::Error` |
| Why | callers match on concrete variants; a boxed dyn error takes that away | heterogeneous errors from the whole graph, nobody matches |
| `?` conversion | `#[from]` per suberror | automatic for any `Error + Send + Sync` |
| Context | variant fields carry the offending value | `.context("…")` |

The rule is per crate, not per repo: a `lib` uses `thiserror` while the binary beside it uses `anyhow`.

Both are dependencies, and a project that has declined the boxed-dyn-error one is not stuck. The binary then uses the same `thiserror` enum shape as its libraries, with one top-level variant per subsystem and `#[from]` on each:

```rust
#[derive(Debug, thiserror::Error)]
enum Error {
    #[error(transparent)] Config(#[from] config::Error),
    #[error(transparent)] Store(#[from] store::Error),
}
```

You lose `.context("…")` and gain a surface a test can match on. The cost is one variant per new failure source, which is the cost the libraries already pay — and a `main` that prints the `source()` chain (`har-layout`) reads about as well as a context trail.

```rust
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad frame {id}")]
    Frame { id: FrameId },
}
```

`std::error::Error` needs only `Display` + `Debug`; `#[from]` is what makes `?` convert instead of `.map_err(..)`. (`har` owns variant granularity and `#[non_exhaustive]`.)

**A wrapping error preserves its cause through `source()`, and says its own piece in `Display` — never both.** `thiserror`'s `#[from]` and `#[source]` wire `source()` for you; a `#[error("io: {0}")]` that also interpolates the cause prints it twice once the application walks the chain. Chain-walking and backtrace rendering belong at the application boundary, not in the library. `std::error::Report` is the std formatter for that job and is **still nightly-only** behind `error_reporter` — a stable binary walks `source()` itself (`har-layout` owns the CLI shape of that output).

## Method naming is a cost contract

| Prefix | Cost | Receiver | Returns |
| --- | --- | --- | --- |
| `as_` | free, a view | `&self` | borrowed (`as_str`, `as_bytes`) |
| `to_` | expensive — allocates or converts | `&self` | owned (`to_string`, `to_vec`) |
| `into_` | variable, consumes the input | `self` | owned (`into_bytes`) |

A caller reads the cost off the name, so a `to_` that is free or an `as_` that allocates is a lie. Getters drop `get_`: `frame.id()`, not `frame.get_id()`; the mutable pair is `id_mut()`. Iterator trio, always all three where they make sense: `iter()` → `&T`, `iter_mut()` → `&mut T`, `into_iter()` → `T`. `for x in collection` desugars to `into_iter`. (`har-layout` owns crate, module, and feature naming.)

## Surface

Start narrow: private → `pub(super)` → `pub(crate)` → `pub(in path)` → `pub`. Narrowing later needs a major bump; widening needs a minor one.

Visibility is only half of it. A `pub` struct with all-public fields commits every field and lets outsiders build it with literal syntax; a `pub` enum commits its variant set and every outside `match` is exhaustive over it. Adding `#[non_exhaustive]` **later** is itself a major break. So decide at introduction: data meant to grow gets private fields plus a constructor and accessors, or `#[non_exhaustive]` from version one, and the docs say which.

A const parameter is part of the public type's identity, so changing one is a major break and designing an API around an unstable const-generic capability commits you to nightly forever. `har-target` owns when a const parameter earns its place and what it costs.

### MSRV

`[package] rust-version = "1.99"` is a support contract Cargo enforces, not a comment. Set it, state the policy for raising it, CI-test that exact toolchain across the supported feature sets, and treat a raise as a versioning and release-note event. "It compiles on the newest compiler" is not an MSRV claim. Resolver 3 is the edition-2024 default and prefers MSRV-compatible dependency versions, which reduces accidental raises but does not test the floor for you. (`har-supply` owns toolchain pinning and the CI shape.)

### Breaking changes

Major:

- removing a public item, or changing a signature
- adding an enum variant without `#[non_exhaustive]`; adding `#[non_exhaustive]` to something already public
- adding a public field to a struct with no private fields
- **tightening a generic bound**, or adding a bound to an existing parameter
- **changing an RPIT capture set**, including by an edition migration
- **changing a layout or representation guarantee** (`repr`, size, alignment) that callers or FFI rely on
- adding a trait item that costs dyn compatibility
- losing dyn compatibility by any other route; adding a blanket impl that can conflict
- changing the license, or the **default features**
- removing a feature, moving public code behind one, or removing a default feature

*Possibly* breaking, because of name collisions and method resolution: new public items, new defaulted trait methods, new inherent methods. Review them rather than waving them through.

Under Cargo's convention the left-most non-zero component is the incompatible one, so for a `0.y.z` crate the **minor** position carries breakage. State the crate's 0.x policy rather than applying post-1.0 intuition to it.

`cargo semver-checks` is evidence, not proof. Its checked feature set is configurable and its defaults skip feature names conventionally used for unstable or internal surfaces, and it cannot see inference, method-resolution, behavioral, layout or MSRV changes. So: define the supported feature matrix, run it with explicit baseline and current feature selections for each supported surface, compile a downstream fixture, and review by hand what a public-API diff cannot prove.

Re-export any dependency whose types appear in your public API: `pub use serde;`. Without it a caller on another version gets trait-bound errors, because `Serialize` at two versions is two unrelated traits and an impl against one does not satisfy the other.

Features are additive and Cargo unifies them across the whole graph — never ship two features that are mutually incompatible, because nothing prevents both being enabled. Never feature-gate a public struct field or trait method; an outside constructor or implementor cannot know what to supply. Never `use somecrate::*` from a crate you do not control — a minor upgrade can add a colliding symbol or a trait method that shadows an inherent one. `use super::*` in a test module is fine.

| Need | Reach for |
| --- | --- |
| Abstract over values | function |
| Abstract over types | generic + trait bound |
| The same code per enum variant or field | `derive` macro |
| Centralize a table that would otherwise scatter | `macro_rules!` |
| Variadic or DSL-shaped call syntax | `macro_rules!` |
| Anything else | not a macro |

Macro costs: a second language to learn, opacity to rustfmt and rust-analyzer, invisible code bloat, poor errors. Never hide a `return` inside a macro, never have one insert references, and prefer a `derive` to a proc macro that emits a type. `cargo expand` shows what one actually produced. (`har-supply` owns proc macros as build-time attack surface.)

## Documentation

Every public item is documented. `# Panics` names the precondition that avoids it, `# Errors` names each failure, `# Examples` where use is not obvious — examples are doc tests, so they cannot rot, and they use `?` rather than `unwrap`. A sealed trait says it is sealed; an open one says outside implementations are supported. Do not restate the signature and do not describe how other code uses the item. Link with intra-doc links `[`Frame`]`; backtick anything that is source. Enforce with `#![warn(missing_docs)]` and `#![deny(rustdoc::broken_intra_doc_links)]`.
