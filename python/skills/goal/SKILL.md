# goal

Session goal state from the kernel. The goal lives host-side, outside the
transcript, so it survives compaction.

```python
await goal.get()                       # objective, status, budgets, remaining
await goal.create("ship phase 6")     # only when explicitly requested
await goal.create("...", token_budget=500_000)
await goal.update("complete")         # or "blocked" after the blocked audit
```

Create a goal only when the user or system explicitly asks for one. `update`
accepts only terminal reports; pause/resume and limits belong to the host.
