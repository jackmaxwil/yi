---
issue: Refs #1670
raise: crate cli +20, crate runtime +22, crate types +2, tests +96, comments +2
---
`yi ask --allow-write DIR`, repeatable, and `permissions.allowWrite` in the config make a directory writable to contained commands and write tools for the run. A headless agent in the workspace sandbox could not write a linked worktree's git directory unless its working directory was that exact worktree, so every build and gate task ran `--yolo`; now the task names the directory and keeps the sandbox. A grant is a kept write rule, so a child, a kernel `bash()` job and the in-process write tools see it through the paths a widened retry already uses. The grant belongs to the run that named it and is not written to the session ledger, so a `--continue` run names the directory again. A directory that does not exist, `$HOME`, `~/.yi` or a credential store is refused with exit 2 before the model runs. Relative paths resolve against the working directory. A `/new` in an interactive run clears the grant with the other session rules.

No `--allow RULE` flag: no gate refused a command in the incident, only the sandbox, and a rule that skips the policy is `--yolo` by another name.
