---
id: security
question: does it work
decider: reader
claim: Risk and rollback
when: {"paths": ["crates/tools/src/sandbox*", "crates/runtime/src/wall*", "crates/runtime/src/permission*", "crates/permission/*", "crates/ai/src/auth*", "crates/oauth/*", "adapters/*", "scripts/*", ".forgejo/*", ".github/*"]}
severity: {"high": "a wall, secret, credential or sandbox boundary gets weaker", "medium": "untrusted text reaches a model or a shell unfenced", "low": "a hardening the change could add"}
refute: {"high": 3, "medium": 1, "low": 1}
---
Look at every trust boundary the change touches: walls and sandboxes, permission decisions,
credentials and tokens, text from outside Yi reaching a model or a shell, anything that pushes,
posts or runs with someone else's authority.
