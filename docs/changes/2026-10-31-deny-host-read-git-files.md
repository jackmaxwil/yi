---
issue: Closes #1929
decision: config.worktree, info, a module's config, alternates, every git dir's worktrees and a .git that is no directory are never writable to a contained process | a planted core.fsmonitor in config.worktree ran on the host's next git status, and a repointed .git reached the same place | drop the entries from Sandbox::for_workspace
raise: tests +204, comments +7, crate runtime +14, crate tools +49
---
A contained process can no longer write the git files the host reads as authority. With
`extensions.worktreeConfig` on, `config.worktree` was writable, and the host's next `git status`
ran a `core.fsmonitor` planted there; a lane's `.git` pointer could send that status to a config
the process wrote. `info`, a submodule's config and `objects/info/alternates` had the same shape.

`worktrees` is denied for every git dir, not only a main checkout's: a lane could write a sibling
lane's admin files, which its own next `git status` runs. A writable root inside a denied path is
spared, which is how a lane keeps its own index in `<trunk>/.git/worktrees/<lane>`.

A contained `git sparse-checkout`, `git config --worktree` and `git worktree add` from a main
checkout are now refused, and `git init` in a directory that is no repository asks once. A
submodule's object store stays writable, so `git submodule update --init` still populates it; its
`config` does not.

`a_spared_writable_root_takes_writes_and_a_spared_path_only_reads` derived its second spelling of
the scratch dir from `/private`, which the shared scratch harness stopped leaving in place.
