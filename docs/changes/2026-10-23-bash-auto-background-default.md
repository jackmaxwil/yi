---
issue: Closes #1437
decision: a bash command still running after 60 s becomes a job by default in every session that outlives its idle (the terminal UI, the console, `yi serve`, `yi rpc`, `yi acp`); `bash.autoBackgroundMs` unset means 60,000 there, `0` turns backgrounding off, and `yi ask` keeps it off unless set (amends D13's default-off) | the owner: "this should be default for everyone with yi, not just my personal config"; on 2026-10-23 `glm-5.3-flash` appended `just check` piped to `tail -5` to a read and passed no `wait`, so the turn sat on "Running…" for the whole gate and then ran it again, and D305's `wait` only helps a model that passes it. `yi ask` exits once its session is idle and nothing in it waits on a job, so a job the default backgrounded there is never heard and outlives the run (#819) | return `configured_auto_background` to `None` when the key is unset and drop its `exits_idle` argument; a config written meanwhile keeps its value, and `0` reads as off on both sides
raise: crate cli +24, tests +56, comments +2
---
A bash command that is still running after 60 seconds now hands the turn back as a job in the
terminal UI, the console and every other long-lived session, not only for users who set
`bash.autoBackgroundMs` (Closes #1437). Before, the key was off unless set, so a model that
passed no `wait` held the turn for a whole build or test suite, up to the 300 s
`timeout_secs`. The result reads `[still running after 60s: now job N, …]` with the D305 text
and a tail of what the command has printed so far, and the job's `<async_result>` starts the
next turn when it exits. A configured value still wins, and `0` turns backgrounding off. A
command whose `timeout_secs` is 60 or less still runs in the turn, since a limit that does not
come before the timeout backgrounds nothing. `yi ask`, and every benchmark run under it, keeps
the old behaviour unless the key is set: it exits once idle, so a job it backgrounded would
finish after the run had ended and nobody would see the result.
