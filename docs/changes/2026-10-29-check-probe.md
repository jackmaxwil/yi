---
issue: Closes #1536
raise: crate cli +20
raise: crate runtime +62
raise: tests +175
raise: comments +4
---
A check whose first program is not on the PATH, or whose shell cannot parse it, is now refused before it runs instead of failing as if the check itself were red. `yi_runtime::goal::probe` asks `sh -n -c` for syntax, then one `sh -c` over the first word of each `yi_permission::command_segments` segment (an assignment prefix is skipped), naming the exit a shell would have given it: 127 for a name not on the PATH, 126 for a path that is not an executable file. Only first words are probed, and the refusal says so, because a compound command's later word can still be there or not. The refusal names the ways out: fix the syntax, install the program, or declare no check.

`yi ask --check CMD` probes the command at startup and exits 2 with that refusal before any provider request or session file, and exits 0 when the command could start. `yi ask` is the only verb that takes it; anything else says so.
