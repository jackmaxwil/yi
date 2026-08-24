# Never

- Never edit generated AGENTS.md/CLAUDE.md or propagated skill directories —
  edit .ruler/ and run `npx @intellectronica/ruler apply`.
- Never open paths named in an Appendix A excise block.
- Never add a dependency absent from YI_DESIGN.md 13.3; deny.toml enforces the
  banned list.
- Never edit a guardrail baseline in the same commit as code.
- Never commit while a gate is red; never commit or push unasked.
- Never modify committed golden fixtures; add new ones beside them.
- Never introduce a YI_* env var without its row in
  scripts/guardrails/baselines/env_vars.json (hard cap 40).
- Never unwrap/expect/panic outside tests; never pass a bare String or u64
  across a crate boundary where a newtype exists.
- Never scaffold ahead of the current phase gate.
