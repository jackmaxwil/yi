---
name: har-supply
description: Rust dependency and unsafe policy — minimal deps, cargo-vet admission control, cargo-deny and cargo-audit, build-script and proc-macro sandboxing, artifact-to-source verification, transitive unsafe, forbid(unsafe_code) posture, toolchain pinning, SBOM and binary inventory, reproducible builds. Load before adding a dependency or auditing the graph.
---

# Dependency and unsafe policy

Every direct dependency is a review obligation over its **entire transitive tree**. One malicious crate anywhere in the graph compromises the whole application — and it does not need to wait until runtime, because `build.rs` and proc macros execute on the build host.

## Before adding a crate

1. Does `std` do it? Does an already-present dependency do it? Is it under ~100 lines to write?
2. Cost of admission — run before `cargo add`, not after:

```
cargo tree --depth 1                        # what you already have
cargo tree --invert <crate> --workspace     # who else pulls it in, and at which versions
cargo tree -e build                         # build scripts and proc macros: arbitrary code at build time
```

`--invert` needs a package spec, and without `--workspace` it answers only for the current member.

3. Verify the exact name and repo URL. Typo-squats differ by one character.
4. Check for `unsafe`. A crate advertising `#![forbid(unsafe_code)]` is a weak signal, not an admission criterion — see below.

A dependency that adds 40 transitive crates to save 30 lines is a bad trade.

## The four questions a dependency gate must answer

Graph shape and advisories are two of them. A high-assurance gate answers all four:

| Question | Tool |
| --- | --- |
| Is this graph shaped acceptably — licenses, duplicates, sources? | `cargo deny` |
| Has anyone published a vulnerability against it? | `cargo audit` |
| **Has the code itself been reviewed, by someone, at a stated criterion?** | `cargo vet` |
| **Does the artifact Cargo downloaded match the source that was reviewed?** | artifact/source reconciliation |

`cargo deny` and `cargo audit` answer nothing about the third and fourth. They are policy and advisory checks, not review.

## cargo-vet: admission control

`cargo vet` maps the resolved dependency graph onto audit records — yours, or ones you explicitly import from an organization you trust — and fails when a version enters the graph with no audit covering it.

```
cargo install --locked --version <pinned> cargo-vet
cargo vet init
cargo vet --locked
```

- Commit `supply-chain/` — the policy, your audits, and the imports — alongside `Cargo.lock`.
- Require **`safe-to-deploy`** for anything in a production subtree; it covers generated and deployed code and implies `safe-to-run`.
- Define a custom criterion (`crypto-reviewed`, `parses-untrusted-input`) for crypto, TLS, and any parser fed by a peer, and require it there.
- `cargo vet init` writes an exemption for everything already present. That set is **not evidence**. Shrink it deliberately, put an expiry on every entry, and require a security owner's approval to add one.
- A delta audit between two versions is usually the affordable unit; reserve full audits for what deserves them.

`cargo-crev` is a useful second signal — cryptographically signed reviews and an explicit web of trust, still actively published — but its assurance is exactly the assurance of the identities you trusted. Import only named, approved reviewers, set a trust depth, and rank it behind your own vet policy.

## Build scripts and proc macros are privileged code

Cargo runs a package's `build.rs` **before** building that package, with your user's privileges: it can read `~/.ssh`, reach the network, and shell out. A proc macro runs inside `rustc` with the same reach. Reviewing the graph does not contain either.

- Run dependency-introduction and release builds in a disposable, least-privilege environment. Deny network egress by default.
- On Linux, `cackle` (`cargo acl`) plus `bubblewrap` enforces per-package API and filesystem permissions, including sandboxing `rustc` so proc macros are contained. Commit `cackle.toml` with a narrowly reviewed permission entry for every build script, and run it in CI.
- Treat it as defence in depth: Cackle's own docs describe evasion routes and non-granular proc-macro permissions.
- `cargo tree -e build` is the inventory. A new entry there is a security review, not a version bump.

The same applies to the build scripts **you** write, and there the rules are about correctness as much as trust:

- No network, no secrets, no time- or environment-dependent generation. A build that is not reproducible is a build whose artifact corresponds to no commit.
- Write only into `OUT_DIR`, and never assume it is clean — it persists between builds.
- Declare every input: `cargo::rerun-if-changed=` for each file read, `cargo::rerun-if-env-changed=` for each variable. An undeclared input is a stale artifact waiting to happen.
- Read `CARGO_CFG_TARGET_OS`, `CARGO_CFG_TARGET_ARCH`, `CARGO_CFG_TARGET_HAS_ATOMIC` — never host `cfg!`, which describes the machine running the script (`har-target`).
- Validate generated source before `include!`ing it, and keep generation deterministic so two builds of one commit produce identical bytes.
- A proc macro has the compiler's file and stdio access and can hang a build. Pin and audit them, bound the input complexity they accept, and test the expansion (`cargo expand`).

## Verify the artifact, not just the repository

The `.crate` file Cargo downloads is not provably the repository you read, and **this question has no mature tool.** Do not let a gate depend on one that is younger than the risk: the admission rules above — exact name, exact repo, pinned version — apply to your own tooling first.

What you can do without one, for every new or updated dependency carrying a build script, a proc macro, native code or a shipped binary: unpack the `.crate`, read `.cargo_vcs_info.json` for the commit it claims, and diff the unpacked tree against that commit. Do it as part of the `cargo vet` delta audit, where you are already reading the diff, and record the verdict with the version. An unverifiable result means **manual review required**, not pass.

Prefer versions published through Trusted Publishing, and publish your own crates that way — OIDC rather than a long-lived token — but treat provenance as a signal about *who published*, never as a code audit.

Identify a publisher from the **crates.io owners**, not from `repository` and never from `authors`. The `repository` field says where source lives; it is not the set of accounts authorized to publish or yank, that set can be users or teams, and it changes over time. `cargo supply-chain publishers` reports it. For crates you own, use team ownership.

## Unsafe posture

Workspace root:

```toml
[workspace.lints.rust]
unsafe_code = "forbid"
```

and in **every** member's manifest:

```toml
[lints]
workspace = true
```

Workspace lints are not implicitly inherited — Cargo warns when a member omits the opt-in, so CI-check for it.

The other mechanism is a crate attribute, `#![forbid(unsafe_code)]` at the top of every `lib.rs` and `main.rs`. Both work, and the trade is where the failure mode sits: the workspace lint is one place and covers a new crate the moment it opts in, but a member that forgets `[lints] workspace = true` is silently unprotected; the attribute cannot be lost that way and is visible in the file a reviewer is already reading, at the cost of one line per crate and nothing stopping a new crate from omitting it. Pick one, and have CI assert the thing your choice cannot guarantee — the opt-in line, or the attribute's presence.

Either way, `forbid` beats `deny`: it cannot be re-enabled by an inner `#[allow]` during a later refactor.

This binds your crates only, never dependencies; Cargo caps lints for non-path dependencies. Roughly a fifth of public crates use `unsafe`, and unsound `unsafe` produces exactly the memory errors Rust otherwise eliminates. To see it:

```
cargo install --locked --version <pinned> cargo-geiger
cargo geiger                      # unsafe counts per crate in the graph
cargo tree -f "{p} {f}"           # feature flags that may enable unsafe paths
```

`cargo-geiger` is a **triage metric, not a verdict** — its own docs say so: it exists "to provide statistical input to auditing" and "is not meant to advise directly whether the code ultimately is truly insecure or not". It counts explicit `unsafe` and can say nothing about safe-looking code, build scripts, proc macros, generated or native code, or a malicious upstream change. Diff the counts and locations across an upgrade and review what moved. Neither a zero count nor a `forbid(unsafe_code)` badge is an admission criterion; `cargo vet` and the sandbox are.

Vet reachable unsafe first — `unsafe` in a code path you never call is lower risk than `unsafe` under your hot loop. Safe abstractions over `unsafe` internals are legitimate; unchecked `unsafe` in your own call graph is not.

**When `unsafe` is unavoidable.** Confine it to one module that does exactly one thing, never wider. Keep that module out from under the workspace `forbid` with a local `deny` rather than weakening the workspace setting. `SAFETY:` on both sides — the block and every caller — naming which side owns which precondition. Review the whole module, not the diff, with two reviewers. Test the unsafe code *and* its clients, plus one test across the boundary. Keep an inventory of every `unsafe` site with its evaluated risk:

```toml
# unsafe-inventory.toml — one entry per site, reviewed at the version named
[[site]]
module   = "crates/ring_buffer/src/raw.rs"
kind     = "UnsafeCell + unsafe impl Sync"
risk     = "high"
reviewed = { by = ["alice", "bob"], commit = "9e3a29d", date = "2026-08-14" }
tests    = ["loom::ring", "miri::ring_roundtrip"]
```

An unlisted `unsafe` is an unreviewed one. `har-unsafe` covers how to read one.

## cargo-deny

One tool, four gates. `deny.toml` at the workspace root:

```toml
[advisories]
yanked = "deny"
unmaintained = "all"              # "workspace" checks direct deps only
unsound = "all"                   # default scopes this to workspace deps too
unused-ignored-advisory = "deny"  # a stale ignore is a silent hole

[bans]
multiple-versions = "deny"
wildcards = "deny"

[licenses]
allow = ["MIT", "Apache-2.0", "BSD-3-Clause", "Unicode-3.0"]

[sources]
unknown-registry = "deny"
unknown-git = "deny"
required-git-spec = "rev"         # an allowed git source must still pin a revision
allow-git = ["https://github.com/example-org/example-dep"]
```

```
cargo install --locked --version <pinned> cargo-deny
cargo deny check
```

`multiple-versions = "deny"` is the highest-value line: duplicate versions mean bloat, divergent behavior between call sites, and two copies to patch when an advisory lands. Grant exceptions by name in `[bans].skip`, with the reason in the commit message, never by weakening the rule:

```toml
[[bans.skip]]
name = "windows-sys"
version = "=0.52.0"
# pulled by `notify` 6.x; drops out when notify 7 lands. Re-check 2026-12.
```

Every `[advisories].ignore` entry carries an expiry and a triage note; `unused-ignored-advisory = "deny"` is what stops them accumulating. In an offline or air-gapped lane, set a short maximum advisory-database staleness rather than letting a cached database age silently.

## cargo-audit

```
cargo install --locked --version <pinned> cargo-audit
cargo audit
```

Scans `Cargo.lock` against the RustSec advisory database. Also flags unmaintained crates. No reachability analysis, so a hit may be unexploitable in your usage — triage each one and record the verdict; do not blanket-ignore. Run in CI on every PR, plus on a schedule, since new advisories land against unchanged code.

## Pin the security tools too

A bare `cargo install cargo-deny` resolves whatever release exists that day, so the meaning of your gate changes between CI runs. `--locked` only honours *that release's* lockfile. Install every decision-making subcommand — `cargo-deny`, `cargo-vet`, `cargo-audit`, `cargo-acl`, `cargo-geiger`, `cargo-auditable`, `cargo-semver-checks`, the SBOM tool — at an approved exact version with `cargo install --locked --version …`, or from a verified prebuilt artifact, and upgrade them through the same reviewed process as dependencies.

## Toolchain, MSRV, and resolution

- Pin the toolchain in `rust-toolchain.toml` — same compiler, same lints, same output for everyone. A toolchain bump is its own commit, with its own full suite run.
- Declare `[package] rust-version` and test that exact floor in CI (`har-api` owns the MSRV contract and what raising it means for semver). Resolver 3, the edition-2024 default, prefers MSRV-compatible dependency versions; it does not test the floor for you.
- Assert `RUSTC_BOOTSTRAP` is unset in CI. It turns a stable toolchain into a nightly one silently, and every guarantee you documented was about the stable one. This is also why an unstable resolver or SBOM feature gets its **own pinned nightly lane**, never a bootstrap escape in the production one.
- **Release-age quarantine.** A lockfile freezes a compromised release exactly as well as a good one, so a same-day typo-squat or account takeover can be pinned in before anyone reports it. Cargo's experimental `-Z min-publish-age` / `registry.global-min-publish-age` filters versions younger than a configured window. Evaluate it in a separately pinned nightly dependency-update job, with security-reviewed exceptions, until it stabilizes.
- Choose the resolver explicitly at the workspace root. Edition 2024 implies resolver 3 (1.84+), whose `resolver.incompatible-rust-versions = "fallback"` is a *preference* for MSRV-compatible versions — a dependency's own requirements can still force a newer release. Resolve and update with the MSRV toolchain in CI so that is visible.

### Edition migration is a semantic review

Edition 2024 is still the current edition, so for most codebases this section is reference material rather than pending work — read it when you adopt 2024, or when the next edition lands.

`cargo fix --edition` is the mechanical half. Run the compatibility lint group, then **read** every rewrite it made, because edition 2024 changed behaviour in four places that are exactly where a high-assurance codebase lives:

| Change | What to re-examine |
| --- | --- |
| `if let` scrutinee and tail-expression **temporary drop scopes** | every lock, guard, and `Drop`-bearing temporary — the unlock point moved (`har-concurrent`) |
| `static mut` references now **denied** | every global; the fix is an atomic, a `OnceLock`, or `&raw mut` plus a real synchronization story (`har-unsafe`) |
| `extern` blocks must be `unsafe extern` | every declared foreign signature, re-validated by a human — the edition did not check them, it only made you write the word |
| `gen` is a reserved keyword | identifiers, macro output |

Add regression tests around locks, destructors and FFI before the migration, not after. Mechanical edition edits are not semantics-preserving in code that has any of the above, and the compiler will not tell you which rewrite changed a release point. Migrate on its own commit, with its own full suite run.

## Reproducible builds

- Commit `Cargo.lock`. Always for binaries; for libraries too once you ship an executable from the workspace.
- Pin git dependencies to a full commit SHA. `rev = "9e3a29d..."` is reproducible; a bare `branch` or bare `git =` silently moves under you and defeats the lockfile's intent for that crate.
- CI builds with `--locked` (and `--offline` where vendored) so a build can never silently resolve a new version.
- `cargo update` is a deliberate act: update, run the full suite, `cargo deny check`, `cargo vet`, commit the lockfile change on its own.
- `cargo vendor` when the build must survive a registry outage or run air-gapped.
- Delete prior artifacts before the build you ship. `Cargo.lock` pins inputs; it does not evict a stale `.rlib`.
- Never edit sources while a release build runs — the artifact then corresponds to no commit.

## What ships: inventory and SBOM

`Cargo.lock` describes the build input. After a RustSec advisory lands, operations needs to know which *deployed binary* contains the affected crate.

```toml
# release profile, via the cargo-auditable subcommand
```

```
cargo install --locked --version <pinned> cargo-auditable
cargo auditable build --release --locked
cargo audit bin target/release/app
```

`cargo-auditable` embeds the dependency list in the binary at negligible cost. Its own documentation says it does **not** protect against supply-chain attacks — it is an inventory, so that a disclosure becomes a query instead of an archaeology project.

Release provenance is the whole set: locked source revision, compiler and tool versions, artifact hash, the embedded inventory, and a signed SPDX or CycloneDX SBOM. Cargo's native `-Z sbom` output is an unstable JSON precursor — pin a nightly lane if you use it.

## Your qualified subset

Name the APIs this project has decided not to use, in `clippy.toml` at the workspace root:

```toml
disallowed-methods = [
    { path = "std::env::set_var", reason = "process-global, unsound alongside threads" },
]
disallowed-types = [
    { path = "std::sync::RwLock", reason = "OS-dependent fairness; Mutex unless reads dominate and contention is measured" },
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

`strip` removes symbols from what you distribute. `overflow-checks` keeps arithmetic bugs loud in production instead of silently wrapping. `har-hot-path` owns the runtime trade behind each knob.

Static linking (`x86_64-unknown-linux-musl`) gives a copy-and-run binary and takes on the patching burden — no system package manager will update a vendored dependency for you. Verify with `ldd`.

## Review checklist for a dependency PR

- `cargo vet` clean at the required criterion, with no new exemption that lacks an owner and an expiry
- New transitive crate count, and which of them run `build.rs` or proc macros — each with a reviewed `cackle.toml` entry
- Artifact reconciled against source for anything with a build script, native code, or a shipped binary
- `cargo deny check` and `cargo audit` clean; no new duplicate versions; license in the allowlist
- Changed `cargo geiger` counts reviewed by location, not by total
- Any new `unsafe` in the graph, whether it is reachable, and an inventory entry if it is yours
- Lockfile change committed on its own; MSRV still satisfied
