---
name: har-layout
description: Rust project shape — flat workspaces, when a crate split pays, ARCHITECTURE.md, crate and module naming, tracing spans and structured logging, metrics and OpenTelemetry, panic policy and crash handling, error and CLI output discipline, serde and config patterns, compile-time budget. Load when adding a module or crate, wiring observability, or when builds got slow.
---

# Project shape

## Layout

- Virtual manifest at the root. Every crate exactly one level deep under `crates/`. **Directory name equals crate name, or differs from it only by one project-wide prefix** — `crates/types` holding `yi_types` is fine and keeps every path short; `crates/frame/store` holding `frame_store` is the thing to avoid. This holds from ten thousand to a million lines.
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

## The architecture document

One durable document, named in the repo's own contributing guide — `ARCHITECTURE.md` at the root is the common spelling, `docs/ARCHITECTURE.md` or a numbered design doc serve the same purpose. What matters is that there is exactly one and that a newcomer is pointed at it. For the reader who has never opened the repo:

- A bird's-eye statement of the problem.
- A **codemap**: coarse module or crate to responsibility, one line each.
- Named files, modules, and types with **no hyperlinks** — links rot, symbol search does not.
- The **invariants**, especially the ones stated as absences.
- The **trust boundaries**, which `har-threat` produces and this file is the home for.

Nothing about module internals. Revisit twice a year, not per commit.

```markdown
# Architecture

A local-first canvas that runs untrusted analysis tools in a sandbox and
renders their output at 120fps.

## Codemap

- `crates/canvas`      — the window, input, and the paint loop. Owns no state that outlives a frame.
- `crates/frame_store` — the document model and its undo log. The only writer to the database.
- `crates/tool_host`   — spawns tools, frames their stdio, and enforces the sandbox profile.
- `crates/protocol`    — wire types shared by `tool_host` and the tools. No dependencies on the others.
- `libs/span_tree`     — published. Layout of nested spans; pure, no IO.

## Invariants

- Nothing under `canvas` touches the database. It reads a prepared snapshot.
- `protocol` depends on nothing in this workspace, so a tool can link it alone.
- The CLI never reads the log directly; it asks `frame_store`.
- No allocation on the steady-state paint path.

## Trust boundaries

- Tool stdio — frames are attacker-chosen bytes. Bounded codec, sandbox profile, no shell.
- The database — rows are exactly as trusted as the tool output that produced them.
- The CLI socket — anything on PATH can connect; the caller is not necessarily a tool we launched.
```

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

Two things change that trade, and both are common:

- **A runner with process-per-test isolation** (`cargo-nextest`, `har-verify` rung 1) removes the sequential-execution half of the cost, leaving only link time. Measure the link cost before consolidating — `cargo test --no-run` as its own CI step is how you see which half is slow.
- **A project that routes tests into lanes by attribute** — `#[ignore]` plus a reason string a script checks, so an expensive or paid test cannot land in the lane a push runs — needs tests findable by file and name. One binary makes that harder, and the lane discipline is worth more than the link time.

- Route cases through one helper — `fn check(input: &str, expect: &str)` — so a signature change touches one function.
- Externalize cases into data files under `tests/data/`; adding a case then adds no code.
- Keep IO out of the computation layer. Layer tests then run in milliseconds regardless of system size, and every layer is testable directly instead of only through the top.
- In CI, run `cargo test --no-run` as its own step so you can see whether compile or execution is the slow half.

What to test and how hard: `har-verify`.

## Panic policy

A binary declares its panic strategy, and the declaration has consequences beyond binary size.

| | `panic = "unwind"` (default) | `panic = "abort"` |
| --- | --- | --- |
| Destructors on panic | run | do **not** run |
| `catch_unwind` | works | catches nothing |
| Test harness | works | unusable — keep `unwind` in the test profile |
| FFI boundary | a panic reaching `extern "C"` is UB unless you catch it | the process is already gone |
| Binary size, unwind tables | larger | smaller |

Write down which one ships, and then make the rest of the design agree with it: nothing that must happen may live only in a destructor (`har-unsafe` owns why that is true even under `unwind`), and every `extern "C"` export converts panics to a status code at the boundary.

Install a panic hook early in `main`, before anything can panic, and decide these four things:

- **What the user sees.** The default message is a backtrace and a file path. For a shipped tool, print something actionable to stderr and point at a report file; `human-panic` is the ready-made version.
- **What is recorded.** Payload, location, thread name, build id, and a backtrace when `RUST_BACKTRACE` allows it — into a file or a crash reporter, never into stdout.
- **What must not be recorded.** A panic message interpolates values. Secrets must not be in one (`har-threat`).
- **Whether the process continues.** A panicking task does not kill an executor and a panicking thread does not kill the process, so a supervised worker can be restarted — but only after you decide what its abandoned state means. Anything holding a lock leaves it poisoned; anything mid-mutation leaves the invariant broken.

`catch_unwind` is a containment boundary at a specific place — a plugin call, a request handler, an FFI export — not a general error mechanism. After catching, discard or restore the state the unwind passed through, and remember `AssertUnwindSafe` is a claim rather than a check.

## Observability

Every row below names a crate, so start with what you get without one. A level enum, one `write!` to a locked `stderr` behind an env check, and the stdout/stderr split further down cover "what happened" for a single-threaded tool completely:

```rust
// No dependency. Enough for lifecycle lines and errors a human reads.
pub fn log(level: Level, args: fmt::Arguments<'_>) {
    if level <= max_level() {                       // read once from the env, cached in a OnceLock
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{level} {args}");    // diagnostics never touch stdout
    }
}
```

**Spans are the thing with no dependency-free equivalent.** Durations and nesting you can hand-roll badly; re-associating interleaved lines from concurrent tasks is what a span id buys, and hand-rolling that is building `tracing` worse. So the decision is binary and worth making deliberately: either the causality is worth a dependency, or the design should not need it — one worker per task with its own prefix, or a counter instead of a trace. Do not land half of it.

Once a crate is admitted (`har-supply` owns that):

| Want | Use |
| --- | --- |
| A library that must not impose a runtime | `log` |
| Flat "what happened" lines, single-threaded tool | `log` + `env_logger` |
| Duration, nesting, or causality of an operation | `tracing` span |
| Concurrent tasks whose lines interleave | `tracing` — spans are the only thing that re-associates them |
| Structured fields consumed by a machine (JSON, OTel) | `tracing` + `tracing-subscriber` layers |
| Rates, saturation, and anything you alert on | **metrics**, below — logs cannot answer these |
| Per-frame paint path | none of them in steady state — a counter, or a span per interaction |

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
    .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "warn,myapp=info".into()))
    .with_writer(std::io::stderr)
    .init();
```

### Metrics

Tracing shows you one operation in detail. It will not tell you that retries have quietly tripled, that a pool is saturated, or that a bounded queue is at its limit — those are rates and gauges, and they are what you alert on. Define them deliberately:

- **What.** Saturation (queue depth, pool in-use, permits held), errors by class, retries, deadline overruns, resource-bound rejections, integrity and authentication failures. Each one is a bound `har-threat` asked you to set — the metric is how you learn it is being hit.
- **Shape.** Every metric states its unit and whether it is monotonic. Counters count, gauges sample, histograms distribute; do not encode a duration as a counter.
- **Labels are the danger.** Never a request id, user id, raw URL or query string, key, token, payload, or arbitrary error text — each one is unbounded cardinality and most are a disclosure. Redact and bucket at the instrumentation site, not in the backend. For database telemetry the dimension is the operation and the collection, never the query text.
- **Conventions.** Where OpenTelemetry semantic conventions cover the domain, use their names; propagate trace context across task and RPC boundaries explicitly, since a spawned task does not inherit it; set span error status deliberately rather than inferring it from a log line.
- **The exporter must not be able to hurt you.** A telemetry endpoint that is slow, full, or down must never block, panic, or unbound-buffer the data path. Bound the queue, drop on overflow, count the drops, and test that path.

Redaction:

- `#[derive(Debug)]` on a type holding a secret leaks it into every log line and every `#[instrument]` span. Hand-write `Debug` to print `[REDACTED]`. Secrets policy generally: `har-threat`.
- Do not log high-frequency fields — full buffers, whole frame trees, per-element positions. A per-frame `info!` at 120fps is 120 formatted lines a second of cost and noise. Log per interaction, or sample.
- Log at the level the reader needs: `error` a human must act on, `warn` degraded but continuing, `info` lifecycle, `debug` developer detail, `trace` firehose.

## Errors reaching a human

- **stdout is the program's data. stderr is its conversation with the human.** Never mix. A porcelain consumer must be able to pipe stdout with no filtering.
- Exit codes: `0` success, `1` the operation failed, `2` the invocation was wrong. Anything finer must be documented and stable — callers branch on it.
- `main` returns `Result`; the `Termination` impl prints `Debug` and exits `1`. For a real message, print the chain to stderr yourself and return the code.
- Print the error **chain**, one line per cause, most specific first. A single rendered string loses the layer that knew the path. Walk `source()` yourself — `std::error::Report` is still nightly (`har-api`).
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

The `flush` in `Drop` swallows its error — flush explicitly. File IO is unbuffered by default; wrap every file you touch more than once in `BufReader`/`BufWriter`. (`har-hot-path` owns when this shows up in a profile; `har-io` owns durability once the bytes must survive a crash.)

## serde and config

- `#[serde(try_from = "String")]` on a validated newtype makes validation part of deserialization, so no unvalidated value can exist.
- `#[serde(default)]` on a required or credential field silently accepts empty. Leave it off and let deserialization fail.
- `#[derive(Default)]` on a config yields `port: 0`, `timeout: 0` — valid values that mean nothing. Write `Default` by hand, or offer none and require `new`.
- A `bool` plus an `Option<T>` describing one thing admits impossible states: `enum Security { Insecure, Ssl { cert: PathBuf } }`.
- Zero-copy: `&'a str` and `&'a [u8]` fields borrow implicitly, but `Cow<'a, str>` needs `#[serde(borrow)]` or it silently deserializes owned and you get none of the benefit.
- Borrowing only works for formats that point into the input buffer — `from_str`, `from_slice`. `from_reader` demands `DeserializeOwned`. Never write `Deserialize<'static>`.
- Config read from disk is config the user can edit; config read from a peer is `har-threat`'s, and needs bounds, not just types.

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

**Lints worth turning on deliberately**, because the default level is not the right one for a workspace of binaries: `dead_code_pub_in_binary` (1.97, allow-by-default) finds `pub` items nothing reaches — in a workspace that publishes nothing, `pub` means "visible to the next crate up", so this is the compiler doing a reachability audit you would otherwise write by hand. `unused_visibilities` (1.94) catches visibility on `const _`. `unreachable_cfg_select_predicates` joined the `unused` group in 1.99, so it bites the moment you adopt `cfg_select!`.

CI: `CARGO_INCREMENTAL=0`, `RUSTFLAGS=-D warnings` — never `#![deny(warnings)]` in source, which breaks the build on every new compiler. Note that `-D warnings` means a compiler upgrade can fail the build on a lint that did not exist before, which is why a toolchain bump is its own commit with its own full run (`har-supply`). `[profile.dev] debug = 0` or `split-debuginfo = "unpacked"` on macOS, and `[profile.dev.build-override] opt-level = 3` so proc macros are optimized even in dev builds.

## Anti-patterns

| Anti-pattern | Fix |
| --- | --- |
| Deep crate chain "for organization" | flat `crates/`, wide graph, modules inside crates |
| Nested dirs (`crates/frame/store`) | one level deep; the dir name is the crate name, give or take one project-wide prefix |
| `serde`/derive in a core crate everything depends on | push it to a leaf at the boundary |
| `tests/a.rs`, `tests/b.rs`, `tests/c.rs` with no lane discipline and a slow link | `tests/it/main.rs` with modules — unless a per-test runner or an attribute-routed lane wants them separate |
| `util`/`helpers`/`common` module | name what it owns, or fold it into the one caller |
| Trait with one implementation | delete the trait, use the concrete type |
| `bool` + `Option<T>` describing one thing | one enum with data in the variants |
| `#[derive(Default)]` producing invalid values | hand-write `Default`, or drop it |
| `#[derive(Debug)]` over a secret | manual `Debug` returning `[REDACTED]` |
| `#[serde(default)]` on a validated field | `#[serde(try_from = "String")]` on a newtype |
| `Cow<'a, str>` in a `Deserialize` struct without `#[serde(borrow)]` | add `#[serde(borrow)]` |
| No panic hook, so users see a backtrace and a path | install one in `main`; decide message, record, redaction, and restart |
| Request id or raw URL as a metric label | bucket and redact at the instrumentation site |
| Telemetry export on the data path with an unbounded queue | bound it, drop on overflow, count the drops |
| Diagnostics on stdout | stderr; stdout is data only |
| `println!` in a loop | lock once, `writeln!` into a `BufWriter`, explicit `flush` |
| `span.enter()` guard held across `.await` | `.instrument(span)` or `#[instrument]` |
| A subscriber installed by a library | libraries emit; the binary installs one |
| `impl AsRef<str>` on every internal fn | `&str` |
| `#![deny(warnings)]` in source | `RUSTFLAGS=-D warnings` in CI |
