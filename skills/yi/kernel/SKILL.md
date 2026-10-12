---
name: kernel
description: >
  Write one program in the kernel instead of many tool calls. Use when a
  change spans three or more sites, when results feed a loop, a count or a
  table, when several questions or commands can run at once, or when a step
  needs the last one's output without you reading it. Do NOT use for one
  read or one edit: call the tool.
covers: tool:ipython
---

# Programs in the kernel

Every tool result you read costs context and a turn. A cell costs one turn and only what it
prints. Reach for a cell when you would otherwise read data just to copy it into the next call.

| The work | As separate calls | As one cell |
|---|---|---|
| Rename a function at twelve call sites | grep, read each file, twelve edits | grep's value, a loop of edits, a second grep that must come back empty |
| Count, rank or tabulate across files | every file read into context | a loop over values, one printed table |
| Three independent questions about the tree | three reader turns | `asyncio.gather` over three `rlm.ask` calls |
| A long build while you keep working | the build blocks the turn | `h = rlm.bash("cargo build")`, other work, then `await h` |

One read or one edit is a tool call, not a cell: the cell adds a boot and a print for nothing.

## Values, not views

`r.text` is the view you would have read; `r.value` is the data it was rendered from. Loop over
the value and never regex the text: grep's view separates context lines with `--` and pages at
the model's cap, its value holds every match as `path`, `line` and `text`.

```python
hits = await yi.tools.grep(pattern=r"\bold_name\(", path="crates")
assert hits.value.cut is None, hits.value.cut
sites = {}
for m in hits.value.matches:
    sites.setdefault(m.path, []).append(m.line)
for path, lines in sites.items():
    view = await yi.tools.read(path=path)
    # build the patch from view.value.tag and the numbered view.value.lines
```

A value's `cut` is `None` when nothing was left out; anything else names what was and how to
get the rest. Check it before treating a value as complete. An edit cites the tag of the read
it was built from, so read again after an edit before editing that file a second time.
`r.data` is the same value as plain JSON, for `json.dumps` or a DataFrame.

## Running calls at once

`asyncio.gather` over reads and greps runs them together. Writes, edits and bash run one at a
time in the order the cell started them, as your own calls do, so gathering them changes neither
speed nor order. Children are where gathering pays:

```python
qs = ["Where is the retry budget set?", "Who calls select_parts?"]
answers = await asyncio.gather(*map(rlm.ask, qs))
```

Await everything a cell starts before the cell ends. A call made by a task that outlives its
cell is refused, and so is one made after an interrupt.

## Recovering from failures

| Raised | Means | Do |
|---|---|---|
| `yi.tools.UnknownTool` | no such tool, or one a cell cannot call | read `.suggestions`; `yi.tools.excluded()` says why `ipython` and a root's `ask_user` are out |
| `yi.tools.InvalidInput` | the arguments failed the tool's schema | `help(yi.tools.<name>)` shows its parameters |
| `yi.Refused` | a rule, the wall or the user said no | do not route around it, by bash or otherwise: report it, or ask |
| `yi.tools.ToolFailure` | the tool ran and failed | `.result.text` is the tool's own message; a stale tag means read again and rebuild the patch |
| `ToolFailure` with `kind == "cancelled"` | the cell had already ended or was interrupted | move the call inside a cell that awaits it |

A cell that raises halfway keeps what its calls did: the edits before the error landed, and the
cell's result lists them. Read that list before running the cell again, since a rerun repeats
every write.

## Other kernels

Your own variables are already in scope. Another kernel's object is
`await rlm.fetch("kernel://<name>/<var>")`, never `yi.tools.read`, which refuses `kernel://`.
