---
name: login
description: >
  Sign in to a provider with `yi login`: Claude subscription, ChatGPT, Cursor, API keys.
  Use when a model call fails for want of a credential or the user asks to log in.
---

Usage: `yi login <provider> [--no-browser]`, `yi logout <provider>`.

- `yi login claude` is the Claude Pro/Max subscription. Open the printed URL, authorize, and paste the `code#state` the page shows. It is stored as `anthropic`, so `anthropic` models use it with no further setup. Needs a person at the terminal: never run it yourself, hand the user the one command.
- `yi login anthropic` stays the API-key path. A stored key and a stored subscription token share the `anthropic` slot, the last login wins.
- `yi login openai-codex` (ChatGPT) and `yi login cursor` are built in. Other OAuth providers need a profile at `~/.yi/oauth/<provider>.json`.
- "token expired; run: yi login <id>" means the refresh token is gone: log in again.
