# Never

- Never edit generated AGENTS.md/CLAUDE.md or propagated skill directories —
  edit .ruler/ and run `npx @intellectronica/ruler apply`.
- Never add a dependency absent from YI_DESIGN.md §18.3; deny.toml enforces the
  banned list.
- Never edit a guardrail baseline in the same commit as code.
- Never commit while a gate is red; never commit or push unasked.
- Never modify committed golden fixtures; add new ones beside them.
- Never cut, clamp, page, filter, or fall back on a model-facing view without a `[…]` row at
  the cut naming kept/total, the cap, and the next call (045-loud-caps.md).
- Never name a Rust item in bare backticks inside a doc comment — it is an
  intra-doc link, or it is not an item.
- Never introduce a YI_* env var without its row in
  scripts/guardrails/baselines/env_vars.json (hard cap 40).
- Never unwrap/expect/panic outside tests; never pass a bare String or u64
  across a crate boundary where a newtype exists.
- Never put MCP sockets, tokens, or an MCP SDK inside the kernel process —
  kernel Python shells out to the one-shot `yi mcp --json` CLI (design §7.6).
