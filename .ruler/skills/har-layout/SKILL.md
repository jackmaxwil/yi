---
name: har-layout
description: Rust project shape — flat workspaces, when a crate split pays, ARCHITECTURE.md, crate and module naming, tracing spans and structured logging, error and CLI output discipline, serde and config patterns, compile-time budget. Load when adding a module or crate, wiring observability, or when builds got slow.
---

# Project shape

## Layout

- Virtual manifest at the root. Every crate exactly one level deep under `crates/`. **Directory name equals crate name.** This holds from ten thousand to a million lines.
- Keep the common prefix in the folder name — `frame_store`, `frame_paint` — rather than nesting `frame/store`. Hierarchy rots; flat does not.
- `version = "0.0.0"` for internal crates that will never be published. Crates with real semver live in `libs/`, and `libs/` must not depend on `crates/`.
- Never flatten `src/` away, even for a one-file crate.
- Ad-hoc scripts go in an `xtask` crate, not shell.
- Each extra binary artifact costs a full link. One multi-command binary beats N binaries.

## When a crate split pays

A module is free. A crate costs a manifest, a version, a link step, and a public API.

| Signal | Split |
| --- | --- |
| Two independent subsystems that would compile in parallel | yes — widens the graph, direct build-time win |
| A boundary you want the compiler to enforce with no back-edge | yes — cross-crate privacy is the only hard wall Rust has |
| `serde`/proc-macro-heavy code you want off the critical path | yes — push it to a leaf |
| Something you will actually publish | yes, into `libs/` with a real version |
| "It feels big", or speculative reuse | no — make it a module |
| Circular need between the two halves | no — you do not have two components |

Shape the graph **wide, not deep**: independent branches compile in parallel, a chain serializes. Derive-heavy and `serde`-heavy code belongs at the leaves nearest the system boundary so intermediate crates never pay proc-macro cost.

## ARCHITECTURE.md

One file at the root, for the reader who has never opened the repo:

- A bird's-eye statement of the problem.
- A **codemap**: coarse module or crate to responsibility, one line each.
- Named files, modules, and types with **no hyperlinks** — links rot, symbol search does not.
- The **invariants**, especially the ones stated as absences: "nothing under `paint` touches the DB", "the CLI never reads the log directly".

Nothing about module internals. Revisit twice a year, not per commit.

## Naming

- Crates and directories: `snake_case`, identical to each other. Modules and files: `snake_case`. Types: `UpperCamelCase`. Consts: `SCREAMING_SNAKE`.
- Single words where a single word exists — `canvas`, `frame`, `gate`, `log`. Full words, never abbreviations: `config`, not `cfg`.
- No `util`, `helpers`, `common`, `misc`, `manager`. A module whose name does not say what it owns is a module with no owner.
- No stutter: `frame::FrameId` is `frame::Id` when the path already carries the noun.
- Features name a capability the user chooses (`tls`, `serde`, `metal`), never an implementation detail. Additive only — a feature that removes or changes behaviour breaks every crate that unifies it.
- Method-name cost prefixes are `har-api`'s.

## Simplicity budget

- Make something generic only when the second implementation exists **today**. Each generic is monomorphized code, compile time, and cognitive load.
- A trait with one implementation is a concrete type wearing a costume. Delete the trait.
- Lifetimes are a late-stage optimization. `Arc`, `Box`, or a clone in a cold path beats an intricate borrow graph; save borrow discipline for the hot path.
- The test for a good abstraction: adding the next feature feels obvious. If explaining it is work, it is at the wrong level.

## Tests as structure

One integration test binary, not many. Cargo links the library once **per test file** and runs the binaries sequentially.

```
tests/
  it/
    main.rs      mod frame; mod gate; mod log;
    frame.rs
    gate.rs
```

Measured on rust-analyzer: 3x faster test compile, 5x smaller artifacts, run time 20s to 13s.

- Route cases through one helper — `fn check(input: &str, expect: &str)` — so a signature change touches one function.
- Externalize cases into data files under `tests/data/`; adding a case then adds no code.
- Keep IO out of the computation layer. Layer tests then run in milliseconds regardless of system size, and every layer is testable directly instead of only through the top.
- In CI, run `cargo test --no-run` as its own step so you can see whether compile or execution is the slow half.

What to test and how hard: `har-verify`.

## Observability

| Want | Use |
| --- | --- |
| A library that must not impose a runtime | `log` |
| Flat "what happened" lines, single-threaded tool | `log` + `env_logger` |
| Duration, nesting, or causality of an operation | `tracing` span |
| Concurrent tasks whose lines interleave | `tracing` — spans are the only thing that re-associates them |
| Structured fields consumed by a machine (JSON, OTel) | `tracing` + `tracing-subscriber` layers |
| Per-frame paint path | neither in steady state — a counter, or a span per interaction |

Spans are intervals, events are points. Both carry structured fields: `?x` records `Debug`, `%x` records `Display`, `field::Empty` reserves a slot to fill later, dotted keys (`session.id`) group.

```rust
#[instrument(skip(self, buffer), fields(frame = %id, len = buffer.len()))]
fn paint(&self, id: FrameId, buffer: &[u8]) -> Result<(), Error> {
    debug!(kind = ?self.kind, "paint");
    Ok(())
}
```

Bare `#[instrument]` records **every** argument with `Debug`. `skip` everything large, everything per-frame, and everything sensitive; add back only the fields you would grep for.

Never hold the guard from `Span::enter` across an `.await` — the span leaks onto whatever task the executor runs next. Use `.instrument(span)` on the future, or `#[instrument]` on the async fn.

Libraries emit only. The **binary** installs exactly one subscriber, early in `main`:

```rust
tracing_subscriber::fmt()
    .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,afterlife=info".into()))
    .with_writer(std::io::stderr)
    .init();
```

Redaction:

- `#[derive(Debug)]` on a type holding a secret leaks it into every log line and every `#[instrument]` span. Hand-write `Debug` to print `[REDACTED]`. Secrets policy generally: `har-threat`.
- Do not log high-frequency fields — full buffers, whole frame trees, per-element positions. A per-frame `info!` at 120fps is 120 formatted lines a second of cost and noise. Log per interaction, or sample.
- Log at the level the reader needs: `error` a human must act on, `warn` degraded but continuing, `info` lifecycle, `debug` developer detail, `trace` firehose.

## Errors reaching a human

- **stdout is the program's data. stderr is its conversation with the human.** Never mix. A porcelain consumer must be able to pipe stdout with no filtering.
- Exit codes: `0` success, `1` the operation failed, `2` the invocation was wrong. Anything finer must be documented and stable — callers branch on it.
- `main` returns `Result`; the `Termination` impl prints `Debug` and exits `1`. For a real message, print the chain to stderr yourself and return the code.
- Print the error **chain**, one line per cause, most specific first. A single rendered string loses the layer that knew the path.
- `--verbose` maps to log level and stacks (`-v`, `-vv`); `RUST_LOG` overrides it. `--quiet` silences everything below `error`.
- `--json` (or `--porcelain`) switches stdout to one machine-readable record per line; diagnostics stay on stderr unchanged. Never make the human format parseable "as well" — pick one per stream.
- Anything slow enough to look hung gets a progress indicator on stderr, disabled when stderr is not a TTY.

## CLI output discipline

`println!` locks stdout **per call**. In a loop, lock once and buffer:

```rust
let out = std::io::stdout();
let mut out = BufWriter::new(out.lock());
for frame in frames {
    writeln!(out, "{}\t{}", frame.id, frame.kind)?;
}
out.flush()?;
```

The `flush` in `Drop` swallows its error — flush explicitly. File IO is unbuffered by default; wrap every file you touch more than once in `BufReader`/`BufWriter`.

## serde and config

- `#[serde(try_from = "String")]` on a validated newtype makes validation part of deserialization, so no unvalidated value can exist.
- `#[serde(default)]` on a required or credential field silently accepts empty. Leave it off and let deserialization fail.
- `#[derive(Default)]` on a config yields `port: 0`, `timeout: 0` — valid values that mean nothing. Write `Default` by hand, or offer none and require `new`.
- A `bool` plus an `Option<T>` describing one thing admits impossible states: `enum Security { Insecure, Ssl { cert: PathBuf } }`.
- Zero-copy: `&'a str` and `&'a [u8]` fields borrow implicitly, but `Cow<'a, str>` needs `#[serde(borrow)]` or it silently deserializes owned and you get none of the benefit.
- Borrowing only works for formats that point into the input buffer — `from_str`, `from_slice`. `from_reader` demands `DeserializeOwned`. Never write `Deserialize<'static>`.

## Compile-time budget

```
cargo build --timings              # critical path, where parallelism dies
cargo llvm-lines | head -20        # monomorphization bloat
cargo machete                      # unused declared deps
cargo tree --duplicate             # two versions of the same crate
```

Read `Cargo.lock`, not `Cargo.toml`: for each entry, name the problem it solves for the user in front of the app. `syn`-dependent proc macros are usually the critical path.

A generic public fn is a thin wrapper that delegates immediately to a non-generic inner fn — one instantiation of the body instead of one per caller type:

```rust
pub fn load<P: AsRef<Path>>(path: P) -> Result<Config, Error> {
    fn inner(path: &Path) -> Result<Config, Error> { ... }
    inner(path.as_ref())
}
```

Take `&str` and `&Path` in application code; `impl AsRef<str>` exists to buy ergonomics across a published boundary, and costs a monomorphized copy per caller type everywhere else. Prefer `&dyn Trait` at a crate boundary where each downstream crate would otherwise monomorphize its own copy.

CI: `CARGO_INCREMENTAL=0`, `RUSTFLAGS=-D warnings` — never `#![deny(warnings)]` in source, which breaks the build on every new compiler. `[profile.dev] debug = 0` or `split-debuginfo = "unpacked"` on macOS, and `[profile.dev.build-override] opt-level = 3` so proc macros are optimized even in dev builds.

## Anti-patterns

| Anti-pattern | Fix |
| --- | --- |
| Deep crate chain "for organization" | flat `crates/`, wide graph, modules inside crates |
| Nested dirs whose names differ from the crate names | one level deep, directory name equals crate name |
| `serde`/derive in a core crate everything depends on | push it to a leaf at the boundary |
| `tests/a.rs`, `tests/b.rs`, `tests/c.rs` | `tests/it/main.rs` with modules |
| `util`/`helpers`/`common` module | name what it owns, or fold it into the one caller |
| Trait with one implementation | delete the trait, use the concrete type |
| `bool` + `Option<T>` describing one thing | one enum with data in the variants |
| `#[derive(Default)]` producing invalid values | hand-write `Default`, or drop it |
| `#[derive(Debug)]` over a secret | manual `Debug` returning `[REDACTED]` |
| `#[serde(default)]` on a validated field | `#[serde(try_from = "String")]` on a newtype |
| `Cow<'a, str>` in a `Deserialize` struct without `#[serde(borrow)]` | add `#[serde(borrow)]` |
| Diagnostics on stdout | stderr; stdout is data only |
| `println!` in a loop | lock once, `writeln!` into a `BufWriter`, explicit `flush` |
| `span.enter()` guard held across `.await` | `.instrument(span)` or `#[instrument]` |
| A subscriber installed by a library | libraries emit; the binary installs one |
| `impl AsRef<str>` on every internal fn | `&str` |
| `#![deny(warnings)]` in source | `RUSTFLAGS=-D warnings` in CI |
