---
name: har-api
description: Rust API and ownership shape — generics vs dyn Trait, standard traits to derive, From/Into and builders, Drop and RAII, lifetimes and the borrow-checker ladder, Rc/RefCell/Cow choices, iterator and combinator idioms, thiserror vs anyhow, visibility, method naming, docs. Load when designing a type, trait, or public interface.
---

# API and ownership shape

`har` decides what a type may **be** — validity, states, failure, arithmetic, panic freedom. This skill decides how it is **shaped, dispatched, borrowed, and handed out**. Where they touch (errors, conversions, `RefCell`), `har` owns the correctness rule and this file owns the ergonomics.

## Traits and dispatch

| Axis | Generic `<T: Trait>` | `dyn Trait` |
| --- | --- | --- |
| Dispatch | static, monomorphized | vtable, two derefs |
| Binary size / compile time | one copy per type, slower build | one copy, faster build |
| Multiple bounds | free (`T: Debug + Draw`) | needs an invented combined trait |
| Upcast to a supertrait | works | does not |
| Heterogeneous collection | impossible | the reason it exists |
| Type known only at runtime | no | yes |
| Constraints | none | no generic methods, no `-> Self` |

Default to generics. Switch to `dyn` for a heterogeneous collection or a type unknown until runtime. Switch for code size or build time **only after measuring**.

Object safety: no generic methods, no method returning `Self` or a `Self`-containing type. `where Self: Sized` exempts one method and keeps the rest of the trait object-safe.

| `impl Trait` position | Means | Use when | Don't |
| --- | --- | --- | --- |
| Argument `fn f(x: impl Draw)` | anonymous generic param; caller cannot turbofish | the bound appears once and is never named | the bound appears twice, or the caller must pick the type |
| Return `fn f() -> impl Iterator<Item = u8>` | one hidden concrete type | returning a closure or an adapter chain | branches return different concrete types — that needs `Box<dyn Trait>` |

Few required methods, many provided ones: implementor cost is the scarce resource, `Iterator` is the shape to copy (one `next`, fifty provided). A provided method may carry its own bound so it appears only for qualifying types. Adding a defaulted method later is non-breaking except for name collisions.

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
| `Display` | never derive; hand-write | only for end-user-facing types. `Debug` is for programmers |

House line: `#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]` on every id newtype, `#[derive(Debug, Clone, PartialEq)]` on every plain data struct.

Three hard don'ts: never implement `Deref` for a non-pointer type (a newtype with `Deref` leaks its whole inner API and cannot take it back); never overload an operator across unrelated types; never implement half an operator set — `Add` + `Neg` obliges `Sub`, and `x - y == x + (-y)` must hold.

## Drop and RAII

Constructor acquires, destructor releases. `Drop::drop` takes `&mut self` and returns `()`, so **a destructor cannot report failure**: expose `fn close(self) -> Result<(), Error>` for fallible release and let `drop` do the best-effort remainder. `x.drop()` does not compile — use `drop(x)` or an inner block. Guards release correctly through early returns and `?`. `Drop` is not guaranteed to run (leak, abort, `forget`), so it is never the only path for cleanup that must happen.

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

## Ownership and borrowing

| Situation | Take |
| --- | --- |
| Parameter, read-only | `&str` / `&[T]` / `impl IntoIterator` |
| Struct field, any doubt | **owned** `String` / `Vec<T>` — a lifetime param infects every holder, transitively |
| Struct field, provably shorter-lived than its source, measured hot path | `&'a T` |
| Usually borrowed, occasionally extended or mutated | `Cow<'a, T>` |
| Small immutable value the borrow checker is fighting over | clone it |

A lifetime is the span from creation to drop **or move**. Elision, in three rules: one input reference gives every output reference its lifetime; several input references with no output reference each get their own; a method taking `&self` gives outputs `self`'s lifetime. `'_` names an elided lifetime without inventing one. `'static` comes from statics, promoted consts, and `Box::leak` — not from "lives a long time".

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

`std::error::Error` needs only `Display` + `Debug`; implement `source()` when the cause is worth walking. `#[from]` is what makes `?` convert instead of `.map_err(..)`. (`har` owns variant granularity and `#[non_exhaustive]`.)

## Method naming is a cost contract

| Prefix | Cost | Receiver | Returns |
| --- | --- | --- | --- |
| `as_` | free, a view | `&self` | borrowed (`as_str`, `as_bytes`) |
| `to_` | expensive — allocates or converts | `&self` | owned (`to_string`, `to_vec`) |
| `into_` | variable, consumes the input | `self` | owned (`into_bytes`) |

A caller reads the cost off the name, so a `to_` that is free or an `as_` that allocates is a lie. Getters drop `get_`: `frame.id()`, not `frame.get_id()`; the mutable pair is `id_mut()`. Iterator trio, always all three where they make sense: `iter()` → `&T`, `iter_mut()` → `&mut T`, `into_iter()` → `T`. `for x in collection` desugars to `into_iter`. (`har-layout` owns crate, module, and feature naming.)

## Surface

Start narrow: private → `pub(super)` → `pub(crate)` → `pub(in path)` → `pub`. Narrowing later needs a major bump; widening needs a minor one. `pub enum` and `pub trait` expose every variant and method at once — struct fields and inherent methods do not.

Breaking: removing a public item, changing a signature, adding an enum variant without `#[non_exhaustive]`, adding a public field to a struct with no private fields, losing object safety, adding a blanket impl that can conflict, changing the license or the **default features**. Non-breaking: new public items, new defaulted trait methods, new inherent methods — each with a name-collision caveat. Check it with `cargo semver-checks`.

Re-export any dependency whose types appear in your public API: `pub use rand;`. Without it a caller on another version gets trait-bound errors, because `RngCore` at two versions is two unrelated types.

Features are additive and Cargo unifies them across the whole graph — never ship two features that are mutually incompatible, because nothing prevents both being enabled. Never feature-gate a public struct field or trait method; an outside constructor or implementor cannot know what to supply. Never `use somecrate::*` from a crate you do not control — a minor upgrade can add a colliding symbol or a trait method that shadows an inherent one. `use super::*` in a test module is fine.

| Need | Reach for |
| --- | --- |
| Abstract over values | function |
| Abstract over types | generic + trait bound |
| The same code per enum variant or field | `derive` macro |
| Centralize a table that would otherwise scatter | `macro_rules!` |
| Variadic or DSL-shaped call syntax | `macro_rules!` |
| Anything else | not a macro |

Macro costs: a second language to learn, opacity to rustfmt and rust-analyzer, invisible code bloat, poor errors. Never hide a `return` inside a macro, never have one insert references, and prefer a `derive` to a proc macro that emits a type. `cargo expand` shows what one actually produced.

## Documentation

Every public item is documented. `# Panics` names the precondition that avoids it, `# Errors` names each failure, `# Examples` where use is not obvious — examples are doc tests, so they cannot rot, and they use `?` rather than `unwrap`. Do not restate the signature and do not describe how other code uses the item. Link with intra-doc links `[`Frame`]`; backtick anything that is source. Enforce with `#![warn(missing_docs)]` and `#![deny(rustdoc::broken_intra_doc_links)]`.
