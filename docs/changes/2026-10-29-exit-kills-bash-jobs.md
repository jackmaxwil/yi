---
version: 0.904.0
issue: Refs #819
raise: crate cli +2, crate runtime +5, crate tools +99, tests +137, comments +3
---
A backgrounded `bash` job outlived the `yi` that started it. The job's watchdog is a thread in the
process, and a normal return from `dispatch` ended the process without signalling it, so a
`sleep 300` or a `./server` kept its process group alive: the model was told the job would be
killed at the next interrupt or turn end, and nothing was left to do it. `Jobs::kill_running` now
sets the kill flag of every running job, the `bash` tool's and the kernel bridge's `exec.spawn`
alike, since at exit no owner is left to release either, and waits up to the existing five-second
`KILL_GRACE` for each job's own watchdog to run `kill_tree` (descendants walked first, so a
re-grouped child dies too). `main` calls it once, after `dispatch` returns and before
`std::process::exit`. `Jobs::retire` keeps a retired owner's still-running job in the map, flagged
and killed, until its reaper settles, so `yi acp`, which retires every session before `main`
sweeps, is waited on too. Only that normal-return path is covered: a `yi` ended by a signal (Ctrl-C
on `yi ask` mid-turn, `kill -TERM`, the serve daemon's SIGKILL of a worker) still leaves its jobs
running, since no signal handler is installed.

A tier-2 journey drives the real binary: `yi ask` starts `echo $$ > job-pgid; exec sleep 300` with
`wait: 5`, the turn ends, and the test asserts the recorded process group is gone by the time the
binary exits.

A second, separate fix rides along: `tools::a_job_wait_returns_as_the_job_settles` raced its 80 ms
auto-background timer against `sleep 0.1` and failed under a full-suite load when the sleep
finished inline; it now retries an inline finish (up to twenty attempts for five backgrounded
waits) and still bounds only the waits that ran. Seen red: the unretried loop failed with
`no job id` in a loaded `just check`.
