---
issue: #2223
ledger=: claude login | 5 | core | S | L | `yi login claude` signs in with a Claude Pro/Max subscription through Claude Code's public client (PKCE, the user pastes the `code#state` the console page shows, JSON token body) and stores it as the `anthropic` credential; Anthropic turns on it send Bearer, `anthropic-beta: oauth-2025-04-20` and the Claude Code identity line first in `system`; a profile file at `anthropic` wins and `yi login anthropic` stays the API-key path
cut: none crates/oauth/src/flow.rs
shares: #2205 adds the Claude OAuth headers beside its credential check in crates/ai/src/auth.rs, no overlapping lines
shares: #2234 adds the Claude OAuth headers in crates/ai/src/auth.rs, away from its relay level code
shares: #2235 edits the OAuth profile section of docs/YI_DESIGN.md, not its channel section
raise: crate ai +7, crate oauth +170, tests +491, over-cap +1, comments +11
---

`yi login claude` opens `claude.ai/oauth/authorize`, asks for the `code#state` the console page shows, and exchanges it as a JSON body at `console.anthropic.com/v1/oauth/token`. The token is stored under `anthropic` and refreshed there. A profile file at `~/.yi/oauth/anthropic.json` replaces the built-in one, and `yi login anthropic` is still the API-key path.
