# Architecture rules

Thirteen crates under crates/, all in the default build; yi-mcp-cli is runtime-gated by `mcp.enabled` config (D36), the kernel is compiled unconditionally and boots lazily on first `ipython` call (D38) — neither is a cargo feature.
Naming law: folder `x/` is crate `yi-x` — enforced by scripts/guardrails/check_manifests.py.

Dependency direction is an allowlist in scripts/guardrails/boundaries.toml: unknown crate =
error, stale entry = error, undeclared internal dep = error. Standing rules (YI_DESIGN.md §2):

- yi-types is the DTO wall: serde shapes only; deps = serde alone; no tokio, fs, net, channels,
  handles, `*Runtime` types, and no workspace-wide error enum.
- yi-loop depends only on yi-types (+ tokio::sync::Notify), stays ≤ 1,000 lines, and its public
  API contains no `Result` — failure is encoded as values.
- yi-tui / yi-acp / yi-cli never depend on yi-tools, yi-ai, or yi-permission directly; nothing
  below yi-cli may depend on yi-mcp-cli.
- subagent / mailbox / lane / wiring / goal / plan / rules / wall / schedule / advisor are
  modules inside yi-runtime, never crates. A module that outgrows the 1,200-line file ceiling
  splits at a seam (subagent -> mailbox for B6/B13, lane for the B11 hand-back and the D117 slot pool, wiring for
  `attach_runtime` and the `wire_*` helpers), never by line count; an inherent `impl` may live
  in the module that owns the seam.
- Every dependency is declared once in [workspace.dependencies]; crate manifests add
  `{ workspace = true }` plus features only. Cargo features exist only where §13.4 declares
  them (check_manifests.py allowlist).
- yi-tools never depends on yi-kernel: the `ipython` tool reaches the kernel through the
  `KernelBridge` capability seam; yi-runtime implements it (KernelService) and owns the
  host-handler vocabulary (HostRegistry, design §6).
- python/yi_runtime is Yi's kernel-side Python package (module name `rlm`). It was seeded
  by copying a reference runtime at phase 4 to de-risk the port; Yi owns it outright and
  evolves it freely — there is no upstream to track. Mechanical facts on edits: keep
  `RUNTIME_READY_CHECK` passing (pinned by crates/kernel/tests/ready_check.rs; the check
  string in bootstrap.rs moves with the package), and any package change moves the runtime
  identity hash, forcing a venv rebuild on next boot.
