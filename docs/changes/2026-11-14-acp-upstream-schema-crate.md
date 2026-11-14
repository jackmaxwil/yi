---
issue: #2224
raise: crate console +21
decision: ACP v2 wire shapes are the upstream `agent-client-protocol-schema` crate, not hand-rolled in yi-types, superseding D328's hand-rolled subset; yi-acp and yi-console depend on it directly and yi-types keeps only Yi's own shapes | the hand-rolled subset tracked a pre-release schema (alpha.5) by hand while the crate is versioned by the protocol's authors and 77 of its 121 shared definitions had already moved; the types-only half pulls 40 crates (anyhow behind a scoped deny.toml wrapper, strum skipped as a duplicate) against 96 for the full runtime crate | revert this change: the old `yi-types::acp` module is in git history and every call site is a mechanical builder-to-struct swap
ledger~: ACP v2 server | hand-rolled v2 subset, conformant to the vendored `schema-v2.0.0-alpha.5` (D328); | the upstream `agent-client-protocol-schema` 1.17.0 v2 types, conformant to the vendored `schema-v2.1.17.0.json` (`acp_protocol::v2_client_drives_a_session_end_to_end`, T2);
cut: none crates/acp/src/lib.rs
cut: none crates/acp/src/permission.rs
cut: none crates/acp/src/update.rs
cut: none crates/acp/tests/acp_unit.rs
cut: none crates/cli/tests/acp_protocol.rs
cut: none crates/console/src/app.rs
cut: none crates/console/src/app/chat.rs
cut: none crates/console/src/app/diffs.rs
cut: none crates/console/src/app/notebook.rs
cut: none crates/console/src/app/port.rs
cut: none crates/console/src/app/review.rs
cut: none crates/console/src/model.rs
cut: none crates/console/src/settings.rs
cut: none crates/console/tests/sessions.rs
cut: crates/types/src/acp.rs | pub struct AcpFrame { | the upstream agent-client-protocol-schema crate owns the ACP wire shapes
cut: none docs/YI_DESIGN.md
cut: none scripts/guardrails/baselines/schemas.lock
shares: #2232 edits YI_DESIGN.md §17.2 and §18.3, not the OAuth profile section
shares: #2231 adds one [workspace.dependencies] row to Cargo.toml, away from its profile and exclude lines
shares: #2208 swaps ACP type paths in console render.rs and settings.rs, no layout change
shares: #2205 swaps ACP type paths in crates/acp/src/lib.rs, not its credential checks
shares: #2167 swaps ACP type paths in crates/acp/src/lib.rs and console app.rs, mechanical only
shares: #2235 edits YI_DESIGN.md §17.2 and §18.3, not its channel section
shares: #2234 swaps ACP type paths in crates/acp/src/lib.rs, away from its relay level code
shares: #2238 edits YI_DESIGN.md §17.2 and §18.3 only
shares: #2236 edits YI_DESIGN.md §17.2 and §18.3, not the OAuth profile section
---
The ACP wire is now the upstream `agent-client-protocol-schema` crate (1.17.0, feature `unstable_protocol_v2`, no `schemars`) in `yi-acp` and `yi-console`. `yi-types::acp` keeps `AcpMeta`, `AcpRoute*`, `AcpForkParams`, `AcpCommitted` and `DaemonLedger*`. `anyhow` is allowed only under that crate in `deny.toml`; `strum` is skipped as a duplicate of ratatui's. The vendored schema moves from alpha.5 to the crate's own `schema/v2/schema.json` at tag v1.17.0, and the strict-client test reads it.
