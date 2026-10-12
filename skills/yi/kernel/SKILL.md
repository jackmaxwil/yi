---
name: kernel
description: >
  Write one program in the kernel instead of many tool calls. Use when
  files feed a loop, a count or a table, when a task's requirements must be
  checked together before stopping, when each edit's new text is computed,
  or when readers or a long command can run while you work. Do NOT use for
  one read, one edit, or what one shell command already does.
covers: tool:ipython
---

# Programs in the kernel

Every tool result you read costs context and a turn. A cell costs one turn and only what it
prints. Reach for a cell when you would otherwise read data just to copy it into the next call.

| The work | As separate calls | As one cell |
|---|---|---|
| Turn a directory of logs, CSVs or JSON into one answer | each file read into context | a loop over the files, one printed table or number |
| Check every requirement of a task before stopping | one command per check, each read and remembered | one `verify()` over a dict of checks, PASS or FAIL per line (the `unattended` skill has the template) |
| Edit many sites where each new text is computed, such as a number doubled or a value looked up | a regex that cannot compute, or one edit per site | grep's value, compute each line, one edit per file from the read's tag |
| Map an unfamiliar tree, library or vendor directory | many reads, the context filling up | `asyncio.gather` over `rlm.ask` readers, one question each |
| A long build, download or test run while you keep working | the command blocks the turn | `h = rlm.bash("make -j8")`, other work, then `await h` |

One read, one edit, or a rename one `sed` covers is a tool call, not a cell: the cell adds a boot
and a print for nothing.

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
