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
await plan.edit_add([{ "title": "...", "acceptance": "..." }])
await plan.edit_reopen("t1")
```

Adding tasks is free; removing a task or weakening acceptance is refused —
that requires the user (expand-only standard).
