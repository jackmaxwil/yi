# plan

The session task DAG from the kernel. The plan lives host-side beside the
goal, outside the transcript, so it survives compaction. Task states are
host-verified: a `done` claim runs the task's `check` and validates
`evidence` against its `schema`; failure blocks the task with the evidence.

```python
await plan.get()                       # tasks, states, frontier (ready ids)
await plan.create([
    {"title": "Parse config", "acceptance": "unit tests in tests/config.rs pass",
     "check": "cargo test -p app --test config"},
    {"title": "Wire flag", "acceptance": "--dry-run is accepted", "deps": ["t1"]},
])
await plan.update("t1", "running")
await plan.update("t1", "done")                       # host runs the check
await plan.update("t2", "blocked", reason="...")       # reason required
await plan.update("t3", "done", reason="ask: ...")     # reason required with no check
await plan.edit_add([{ "title": "...", "acceptance": "..." }], for_task="t1")
await plan.edit_reopen("t1")
await plan.split("t1", [                              # only after t1's check stays red
    {"title": "Parse the header", "acceptance": "header cases pass",
     "check": "cargo test -p app header", "writes": ["header"]},
    {"title": "Ask which dialect", "acceptance": "dialect named by the user"},
])
```

Adding tasks is free; removing a task or weakening acceptance is refused —
that requires the user (expand-only standard).

After two red checks the next `done` claim is refused without running the
check. One structural move buys exactly one more claim, for the one task it
names: `split` the task, `split` its parent (what an unsplittable subtask
uses), `edit_add` an investigation task with `for_task`, or `edit_reopen` a
task this one assumes. A move that names nothing buys nothing.
