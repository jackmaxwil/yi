# Architecture rules

Thirteen crates under crates/ (twelve default + yi-mcp-cli behind cargo feature `mcp`).
Naming law: folder `x/` is crate `yi-x` — enforced by scripts/guardrails/check_manifests.py.

Dependency direction is an allowlist in scripts/guardrails/boundaries.toml: unknown crate =
error, stale entry = error, undeclared internal dep = error. Standing rules (YI_DESIGN.md §2):

- yi-types is the DTO wall: serde shapes only; deps = serde alone; no tokio, fs, net, channels,
  handles, `*Runtime` types, and no workspace-wide error enum.
- yi-loop depends only on yi-types (+ tokio::sync::Notify), stays ≤ 1,000 lines, and its public
  API contains no `Result` — failure is encoded as values.
- yi-tui / yi-acp / yi-cli never depend on yi-tools, yi-ai, or yi-permission directly; nothing
  below yi-cli may depend on yi-mcp-cli.
- subagent / schedule / advisor are modules inside yi-runtime, never crates.
- Every dependency is declared once in [workspace.dependencies]; crate manifests add
  `{ workspace = true }` plus features only. Cargo features exist only where §13.4 declares
  them (check_manifests.py allowlist).
