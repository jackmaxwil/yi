---
issue: Closes #1929
decision: config.worktree, info, alternates, a submodule's host-read files, a .git that is no directory and every other lane's admin dir are never writable to a contained process | a planted core.fsmonitor in config.worktree ran on the host's next git status, and a repointed .git reached the same place | drop the entries from Sandbox::for_workspace
raise: over-cap +3
growth: +235 a directory seal (Sealed: the denied directory and the roots it opens), its bindings, its policy rule and its predicate arm, plus a module walk that visits 256 directories instead of every object store, and five tests; the per-name enumeration it replaces was itself the hole
---
A contained process can no longer write the git files the host reads as authority. With
`extensions.worktreeConfig` on, `config.worktree` was writable, and the host's next `git status`
ran a `core.fsmonitor` planted there; a lane's `.git` pointer could send that status to a config
the process wrote. `info`, a submodule's config, `objects/info/alternates` and a sibling lane's
admin dir had the same shape. Work inside a submodule still commits: only its host-read files are denied.

The wall sits on the two directories that hold those names, `<git>/worktrees` and `<git>/modules`,
not on the names one build found there: a lane keeps its own admin dir and the module dirs it
already has, and an admin dir or a module created after the profile is built is under the seal.
`--allow-write` is judged at or below a guarded path only, so granting the checkout root in a
repository is taken again; a refusal still names the tree an approval would widen. The deny
outranks every grant.

A contained `git sparse-checkout`, `git config --worktree` and `git worktree add` from a main
checkout are now refused, and `git init` in a directory that is no repository asks once.
