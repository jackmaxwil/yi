---
version: 0.1072.0
decisions: D603, D604
issue: Refs #2257
decision: Yi Desktop lands as `desktop/`, its own Cargo workspace, after a staged build on the owner's pinned Rune fork, keeps its own gate, v1 uses today's server unchanged, and the app owns its direct local integrations and Luau panes | the owner chose an in-tree native Rust app, a Rune fork that pulls upstream and may take small patches, and a lean UI-first v1, Yi's dependency cap and review rules make a separate staging repository necessary | do not import the staged tree
decision: Yi Desktop's command, approval, restart, project, checkout, shell, language-server, browser, window, plugin and v1-scope defaults are the settled choices in its staging design | the owner delegated behavior absent from the mockup under "I choose, you veto" | change the staging design before import
---
`docs/desktop/` documents Yi Desktop, the native Mac client of `yi serve` built on Rune in a staging repository: what it does, how it is built, the three dogfood rounds with a real model and what each fixed, the measured numbers, the owner's binding answers, the canonical decision-log entries, and the 25 server asks filed for it (#2258 to #2282). No code lands here; the import is #2257.
