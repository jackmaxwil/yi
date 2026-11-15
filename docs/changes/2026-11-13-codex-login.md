---
version: 0.1056.0
decisions: D600
issue: #2221
decision: provider `openai-codex` is compiled: its client id, authorize and token endpoints, loopback redirect `http://localhost:1455/auth/callback`, scopes and extra authorize params live in the binary as a built-in `oauth-code` profile, like `cursor` (revises D191 for `cursor` and `openai-codex`; a profile at `~/.yi/oauth/openai-codex.json` still wins) | the owner asked for `yi login codex`, a ChatGPT Plus/Pro subscription login, and the Codex CLI's public client registers exactly that redirect, so a user cannot supply a working profile without copying the identity by hand | delete `crates/oauth/src/codex.rs` and its registry lookup, which restores D191 for everything but `cursor`
ledger=: openai-codex login | 5 | core | S | L | `yi login openai-codex` signs in with a ChatGPT Plus/Pro subscription through the Codex CLI's public client on a fixed `localhost:1455` loopback, reads the `chatgpt-account-id` from the id_token into the credential and keeps it across refresh; a profile file at the same id still wins
---

`yi login openai-codex` is a built-in login. It opens `auth.openai.com` with PKCE and the Codex CLI's extra authorize parameters, exchanges the code, stores the access and refresh tokens, and keeps the ChatGPT account id from the id_token so each request carries `chatgpt-account-id`. The loopback port is fixed at 1455 because the client's registered redirect is fixed. The "profile you supplied" warning now prints only when a profile file exists.
