---
name: har-supply
description: Rust dependency and unsafe policy — minimal deps, cargo-deny and deny.toml, cargo-audit, vetting transitive unsafe, forbid(unsafe_code) posture, reproducible builds. Load before adding a dependency or auditing the graph.
---

# Dependency and unsafe policy

Every direct dependency is a review obligation over its **entire transitive tree**. One malicious crate anywhere in the graph compromises the whole application.

## Before adding a crate

1. Does `std` do it? Does an already-present dependency do it? Is it under ~100 lines to write?
2. Cost of admission — run before `cargo add`, not after:

```
cargo tree --depth 1              # what you already have
cargo tree -i <crate>             # who else pulls it in, and at which versions
cargo tree -e build               # build scripts and proc macros: arbitrary code at build time
```

3. Verify the exact name and repo URL. Typo-squats differ by one character.
4. Check for `unsafe`. Prefer crates advertising `#![forbid(unsafe_code)]`.

A dependency that adds 40 transitive crates to save 30 lines is a bad trade.

## Unsafe posture

Workspace root:

```toml
[lints.rust]
unsafe_code = "forbid"
```

`forbid` beats `deny`: it cannot be re-enabled by an inner `#[allow]` during a later refactor.

This binds your crates only, never dependencies. Roughly a fifth of public crates use `unsafe`, and unsound `unsafe` produces exactly the memory errors Rust otherwise eliminates. To find it:

```
cargo geiger                      # unsafe counts per crate in the graph
cargo tree -f "{p} {f}"           # feature flags that may enable unsafe paths
```

Vet reachable unsafe first — `unsafe` in a code path you never call is lower risk than `unsafe` under your hot loop. Safe abstractions over `unsafe` internals are legitimate; unchecked `unsafe` in your own call graph is not.

**When `unsafe` is unavoidable.** Confine it to one module that does exactly one thing, never wider. `SAFETY:` on both sides — the block and every caller — naming which side owns which precondition. Review the whole module, not the diff, with two reviewers. Test the unsafe code *and* its clients, plus one test across the boundary. Keep an inventory of every `unsafe` site with its evaluated risk; an unlisted `unsafe` is an unreviewed one. `har-unsafe` covers how to read one.

## cargo-deny

One tool, four gates. `deny.toml` at the workspace root:

```toml
[advisories]
yanked = "deny"
unmaintained = "workspace"

[bans]
multiple-versions = "deny"
wildcards = "deny"

[licenses]
allow = ["MIT", "Apache-2.0", "BSD-3-Clause", "Unicode-3.0"]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
allow-git = ["https://github.com/zed-industries/zed"]
```

```
cargo install cargo-deny
cargo deny check
```

`multiple-versions = "deny"` is the highest-value line: duplicate versions mean bloat, divergent behavior between call sites, and two copies to patch when an advisory lands. Grant exceptions by name in `[bans].skip`, with the reason in the commit message, never by weakening the rule.

## cargo-audit

```
cargo install cargo-audit
cargo audit
```

Scans `Cargo.lock` against the RustSec advisory database. Also flags unmaintained crates. No reachability analysis, so a hit may be unexploitable in your usage — triage each one and record the verdict; do not blanket-ignore. Run in CI on every PR, plus on a schedule, since new advisories land against unchanged code.

## Reproducible builds

- Commit `Cargo.lock`. Always for binaries; for libraries too once you ship an executable from the workspace.
- Pin the toolchain in `rust-toolchain.toml` — same compiler, same lints, same output for everyone.
- Pin git dependencies to a full commit SHA. `rev = "9e3a29d..."` is reproducible; a bare `branch` or bare `git =` silently moves under you and defeats the lockfile's intent for that crate.
- CI builds with `--locked` (and `--offline` where vendored) so a build can never silently resolve a new version.
- `cargo update` is a deliberate act: update, run the full suite, `cargo deny check`, commit the lockfile change on its own.
- `cargo vendor` when the build must survive a registry outage or run air-gapped.
- Delete prior artifacts before the build you ship. `Cargo.lock` pins inputs; it does not evict a stale `.rlib`.
- Never edit sources while a release build runs — the artifact then corresponds to no commit.
- Assert `RUSTC_BOOTSTRAP` is unset in CI. It turns a stable toolchain into a nightly one silently, and every guarantee you documented was about the stable one.
- A toolchain bump is its own commit, with its own full suite run.

## Trusted publishers

For security-critical categories (crypto, TLS, serialization of untrusted input), allowlist the publishing organization for **direct** dependencies — `RustCrypto`, `rustls`, `aws-lc-rs`. Publishers you trust pick their own transitive deps; that is the point of the trust boundary.

Identify a publisher from the `repository` URL, not the `authors` field — `authors` is free text and trivially spoofed.

## Your qualified subset

Name the APIs this project has decided not to use, in `clippy.toml` at the workspace root:

```toml
disallowed-methods = [
    { path = "std::env::set_var", reason = "process-global, unsound alongside threads" },
]
disallowed-types = [
    { path = "std::sync::RwLock", reason = "writer starvation; Mutex unless reads dominate" },
]
```

Every `#[allow(...)]` carries its justification in the same commit — a deviation should cost a sentence, not nothing. `-D warnings` is never lowered to `-W` or `-A` to get a build green; severity only goes up.

## Release build

```toml
[profile.release]
lto = true
codegen-units = 1
strip = true
overflow-checks = true
```

`strip` removes symbols from what you distribute. `overflow-checks` keeps arithmetic bugs loud in production instead of silently wrapping.

Static linking (`x86_64-unknown-linux-musl`) gives a copy-and-run binary and takes on the patching burden — no system package manager will update a vendored dependency for you. Verify with `ldd`.

## Review checklist for a dependency PR

- New transitive crate count, and which of them run `build.rs` or proc macros
- `cargo deny check` and `cargo audit` clean
- No new duplicate versions
- License in the allowlist
- Any new `unsafe` in the graph, and whether it is reachable
