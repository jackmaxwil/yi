---
issue: Closes #819
raise: crate cli +2, crate runtime +5, crate tools +46, tests +82, comments +2
---
A backgrounded `bash` job outlived the `yi` that started it. The job's watchdog is a thread in the
process, and every exit path ended the process without signalling it, so a `sleep 300` or a
`./server` kept its process group alive under the shell's `nohup`-less parent: the model was told
the job would be killed at the next interrupt or turn end, and nothing was left to do it. The job
registry now remembers the process group each backgrounded command was spawned into, and
`Jobs::kill_running` asks every running poller job to stop, signals those groups directly, and
waits up to the existing five-second `KILL_GRACE` for their reapers to settle. `main` calls it
once, after `dispatch` returns and before `std::process::exit`, so `yi ask`, `yi -p`, `yi tui`,
`yi console`, `yi rpc` and a `yi serve` worker all leave no job behind; the daemon's supervisor
owns no jobs, and a worker that exits itself runs the same path. `rustix` stays out of `yi-tools`
(§18.3): the group is signalled through `/bin/sh`'s builtin, the same route `kill_tree` already
takes, because slim images ship no `kill(1)`.

A tier-2 journey drives the real binary: `yi ask` starts `echo $$ > job-pgid; exec sleep 300` with
`wait: 5`, the turn ends, and the test asserts the recorded process group is gone by the time the
binary exits.
