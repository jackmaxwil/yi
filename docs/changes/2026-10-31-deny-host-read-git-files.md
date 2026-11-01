---
issue: Closes #1929
decision: config.worktree, info, modules, alternates, a .git that is no directory and every other lane's admin dir are never writable to a contained process | a planted core.fsmonitor in config.worktree ran on the host's next git status, and a repointed .git reached the same place | drop the entries from Sandbox::for_workspace
raise: tests +181, comments +7, crate tools +34, crate runtime +49
---
A contained process can no longer write the git files the host reads as authority. With
`extensions.worktreeConfig` on, `config.worktree` was writable, and the host's next `git status`
ran a `core.fsmonitor` planted there; a lane's `.git` pointer could send that status to a config
the process wrote. `info`, `modules`, `objects/info/alternates` and a sibling lane's admin dir
had the same shape.

A contained `git sparse-checkout`, `git config --worktree`, a commit inside a submodule and `git worktree add` from a main
checkout are now refused, and `git init` in a directory that is no repository asks once.
