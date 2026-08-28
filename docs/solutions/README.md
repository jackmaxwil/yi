# Solutions

Distilled reference material for working on Yi. The design docs remain the law
(docs/YI_DESIGN.md deep design, docs/ARCHITECTURE.md map + decision log); these
files are the derived, readable views. Regenerate the ADRs from the decision log
when it changes rather than editing them by hand.

- [architecture.md](architecture.md) - the system in one page
- [coding-practices.md](coding-practices.md) - how code is written and gated here
- [comment-style.md](comment-style.md) - typed comment referents and named grants (D55)
- [adr/](adr/) - one architecture decision record per decision-log row

## ADR index

- [D1](adr/d1.md) - ACP v2 only
- [D2](adr/d2.md) - Pi RPC kept
- [D3](adr/d3.md) - model auto-review deferred
- [D4](adr/d4.md) - daemon = ACP-router supervisor + worker-per-root, ACP v2 both hops
- [D5](adr/d5.md) - board deferred
- [D6](adr/d6.md) - goals kept
- [D7](adr/d7.md) - branch summarization deferred
- [D8](adr/d8.md) - CompactionCheck deferred
- [D9](adr/d9.md) - mcp = cargo feature of yi-cli
- [D10](adr/d10.md) - best-of-N = multi-agent judge-selection pattern only
- [D11](adr/d11.md) - hashline registers kept, instrumented
- [D12](adr/d12.md) - dill snapshot kept (4b)
- [D13](adr/d13.md) - reduce launch set = generic + engine + cargo/git/grep/error-stream
- [D14](adr/d14.md) - Pi extensibility = wire hook bridge + Bun sidecar
- [D15](adr/d15.md) - G1 closed
- [D16](adr/d16.md) - pi-ai provider pass-through via sidecar
- [D17](adr/d17.md) - Pi-differential testing
- [D18](adr/d18.md) - bidirectional session handoff
- [D19](adr/d19.md) - advisor reviews the emitted work log, not actions-only
- [D20](adr/d20.md) - bridge wire = JSON-RPC 2.0 reusing the ACP codec
- [D21](adr/d21.md) - compaction logic ports from prime only
- [D22](adr/d22.md) - one token estimator (fx `StreamingEstimator`) serves P3 and streami...
- [D23](adr/d23.md) - jcode-derived guardrails
- [D24](adr/d24.md) - AA-index benchmark integration
- [D25](adr/d25.md) - codex ports
- [D26](adr/d26.md) - codex anti-lessons
- [D27](adr/d27.md) - OMP-derived feature admission
- [D28](adr/d28.md) - advisor two-tier enable
- [D29](adr/d29.md) - retry after first byte is safe and adopted
- [D30](adr/d30.md) - codex pass-2 batch
- [D31](adr/d31.md) - repo plumbing from ref evidence
- [D32](adr/d32.md) - Pi session compat targets the v4 mutation log
- [D33](adr/d33.md) - yi rpc writes v4 only; phase-2 exit narrows to RPC protocol tests
- [D34](adr/d34.md) - OpenRouter as the third native provider; no built-in default model
- [D35](adr/d35.md) - Phase 2b scope trims (registers, per-segment decisions, minimal holds)
- [D52](adr/d52.md) - completion claims are host-verified
- [D53](adr/d53.md) - the plan is a session-store fact
- [D54](adr/d54.md) - triggered rules are a deterministic layer that speaks
- [D55](adr/d55.md) - comment referents are typed, comment grants are named
- [D56](adr/d56.md) - plan transitions are the advisor's review moments
- [D57](adr/d57.md) - Direct OpenAI is a real openai-responses adapter
- [D58](adr/d58.md) - agent messages carry a provenance envelope, not the assistant role
- [D59](adr/d59.md) - advisor promotion writes a rule file, not a permission pattern
- [D60](adr/d60.md) - a tool result carries a typed record of what it did
- [D61](adr/d61.md) - an edit renders its diff in Normal mode, and diff rows wrap
- [D62](adr/d62.md) - the kernel cell renders from its own record
- [D63](adr/d63.md) - syntax highlighting is a hand-rolled scanner, not syntect
- [D64](adr/d64.md) - a finished read-only call is deferred, not committed
- [D65](adr/d65.md) - the subagent family gets a popup, and spawn attribution is observed
- [D66](adr/d66.md) - every animated glyph derives from one clock

D36-D51 have no ADR yet; ARCHITECTURE.md is the record for those rows.
