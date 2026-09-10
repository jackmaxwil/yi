# memory

Notes from earlier sessions, written by the host: one markdown file per fact,
`MEMORY.md` as the index. The index loads at session start; the template for a
new note is in that block.

```python
await memory.read("buildhost-tmp-is-ram")      # a note's body, by name or hook
await memory.save("""---
name: buildhost-tmp-is-ram
description: Buildhost /tmp is RAM; never scratch there
type: feedback
---
The fact.

**Why:** the incident.

**How to apply:** the rule next time.
""")
await memory.forget("buildhost-tmp-is-ram")    # a wrong note
```

Save after a user correction, an incident, or a verified success, never every
turn; three saves per session, root session only. `scope="global"` keeps a note
for every repository.
