# Architecture rules

Sixteen crates under crates/, all in the default build (yi-tui and yi-console behind yi-cli's default `tui` feature); yi-mcp-cli is runtime-gated by `mcp.enabled` config (D36), the kernel is compiled unconditionally and boots lazily on first `ipython` call (D38) — neither is a cargo feature.
Naming law: folder `x/` is crate `yi-x` — enforced by scripts/guardrails/check_manifests.py.

Dependency direction is an allowlist in scripts/guardrails/boundaries.toml: unknown crate =
error, stale entry = error, undeclared internal dep = error. Standing rules (YI_DESIGN.md §2):

- yi-types is the DTO wall: serde shapes only; deps = serde, serde_json, sha2, thiserror; no tokio, fs, net, channels,
  handles, `*Runtime` types, and no workspace-wide error enum.
- yi-loop depends only on yi-types (+ tokio::sync::Notify), and `run_loop` is infallible:
  failure is encoded as values; `AgentTool::validate` is its one `Result` (§2).
- yi-tui / yi-console / yi-acp / yi-cli never depend on yi-tools, yi-ai, or yi-permission directly; nothing
  below yi-cli may depend on yi-mcp-cli.
- subagent / mailbox / lane / wiring / goal / plan / rules / wall / schedule / advisor are
  modules inside yi-runtime, never crates. A module that outgrows the 1,200-line file ceiling
  splits at a seam (subagent -> mailbox for envelopes, lane for the worktree hand-back and the D119 slot pool, wiring for
  `attach_runtime` and the `wire_*` helpers), never by line count; an inherent `impl` may live
  in the module that owns the seam.
- Every dependency is declared once in [workspace.dependencies]; crate manifests add
  `{ workspace = true }` plus features only. Cargo features exist only where §18.4 declares
  them (check_manifests.py allowlist).
- yi-tools never depends on yi-kernel: the `ipython` tool reaches the kernel through the
  `KernelBridge` capability seam; yi-runtime implements it (KernelService) and owns the
  host-handler vocabulary (HostRegistry, design §9).
- python/yi_runtime is Yi's kernel-side Python package (import packages `rlm` and `yi`), Yi's
  own code with no upstream to track. Mechanical facts on edits: keep
  `RUNTIME_READY_CHECK` passing (pinned by crates/kernel/tests/ready_check.rs; the check
  string in bootstrap.rs moves with the package), and any package change moves the runtime
  identity hash, forcing a venv rebuild on next boot.
