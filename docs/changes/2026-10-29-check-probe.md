---
version: 0.810.0
issue: Closes #1536
raise: crate cli +20, crate runtime +73, tests +228, comments +5
---
A check whose first program is not on the PATH, or whose shell cannot parse it, is now refused before it runs instead of failing as if the check itself were red. `yi_runtime::goal::probe` asks `sh -n -c` for syntax, then one `sh -c` over the first word of each `yi_permission::parse` segment (an assignment prefix is skipped), naming the exit a shell would have given it: 127 for a name not on the PATH, 126 for a path that is not an executable file. Only first words are probed, and the refusal says so, because a compound command's later word can still be there or not. The refusal names the ways out: fix the syntax, install the program, or declare no check.

The probe runs at the head of `goal::run_check`, in the directory the check runs in, so goal completion, the discovery drain, plan finish and the mailbox all refuse an unrunnable check instead of reporting it red. A first word the shell expands (`$CARGO`, `"$VENV/bin/python"`, a backtick) is not probed.

`yi ask --check CMD` probes the command at startup, in the `--cwd` directory, and exits 2 with that refusal before any provider request or session file, and exits 0 when the command could start. `yi ask` is the only verb that takes it; anything else says so.
