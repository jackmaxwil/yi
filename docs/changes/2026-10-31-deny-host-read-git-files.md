---
issue: Closes #1929
decision: config.worktree, info, alternates, a submodule's host-read files, a .git that is no directory and every other lane's admin dir are never writable to a contained process | a planted core.fsmonitor in config.worktree ran on the host's next git status, and a repointed .git reached the same place | drop the entries from Sandbox::for_workspace
raise: tests +183, comments +8, crate tools +53, crate runtime +30
---
A contained process can no longer write the git files the host reads as authority. With
`extensions.worktreeConfig` on, `config.worktree` was writable, and the host's next `git status`
ran a `core.fsmonitor` planted there; a lane's `.git` pointer could send that status to a config
the process wrote. `info`, a submodule's config, `objects/info/alternates` and a sibling lane's
admin dir had the same shape. Work inside a submodule still commits: only its host-read files are denied.

A contained `git sparse-checkout`, `git config --worktree` and `git worktree add` from a main
checkout are now refused, and `git init` in a directory that is no repository asks once.
