---
name: ipython
description: >
  Write one program in the kernel instead of many tool calls. Use when
  files feed a loop, a count or a table, when a task's requirements must be
  checked together before stopping, when each edit's new text is computed,
  when readers or a long command can run while you work, or when a log is
  too big to read. Also: the kernel's walls (sandbox, approval, an
  environment fixed at boot), what a restart or a compaction keeps, and
  recovery from a failed call or cell. Do NOT use for one read, one edit,
  or what one shell command already does: that is a tool call.
covers: tool:ipython
---

# ipython

Every tool result you read costs context and a turn. A cell costs one turn and only what it
prints. Reach for a cell when you would otherwise read data just to copy it into the next call.

| The work | As separate calls | As one cell |
|---|---|---|
| Turn a directory of logs, CSVs or JSON into one answer | each file read into context | a loop over the files, one printed table or number |
| A bash result cut to `[full output: path]`, or a log you will query twice | the cut view read again and again | open the path in a cell, filter it, print only what decides |
| Check every requirement of a task before stopping | one command per check, each read and remembered | one `verify()` over a dict of checks, PASS or FAIL per line (the `unattended` skill has the template) |
| Edit many sites where each new text is computed, such as a number doubled or a value looked up | a regex that cannot compute, or one edit per site | grep's value, compute each line, one edit per file from the read's tag |
| Map an unfamiliar tree, library or vendor directory | many reads, the context filling up | `asyncio.gather` over `rlm.ask` readers, one question each |
| A long build, download or test run while you keep working | the command blocks the turn | `h = rlm.bash("make -j8")`, other work, then `await h` |

One read, one edit, or a rename one `sed` covers is a tool call, not a cell: the cell adds a boot
and a print for nothing. Counts, sums, set differences and tables are computed and printed; the
answer quotes the printed value. State lives in variables; bash and the kernel share the working
directory and nothing else, so a file in the tree is how a value crosses between them.

## Values, not views

`yi.tools.<name>(...)` calls the session's own tool from a cell, judged as your own call is.
`r.text` is the view you would have read; `r.value` is the data it was rendered from, or `None`
for a tool that has none. Loop over the value and never regex the text: grep's view separates
context lines with `--` and pages at the model's cap, its value holds every match as `path`,
`line` and `text`.

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

The values of `grep`, `read` (a file, a directory or a glob) and `bash` carry `cut`: `None` when
nothing was left out, otherwise what was and the call that gets the rest. Check it before treating
a value as complete. Keys keep the wire's camelCase (`exitCode`, `timedOut`, `totalLines`), and
`r.data` is the same value as plain JSON, for `json.dumps` or a DataFrame.

An edit cites the tag of the read it was built from, and its own value lists each file's new tag
in `files`. Line numbers move with an edit, so read again before editing that file a second time.

## Running calls at once

`asyncio.gather` over reads and greps runs them together. A call that changes something (a write,
an edit, a bash command that is not read-only, a grep that applies a replacement) runs one at a
time, and a read waits while it runs, as your own calls do, so gathering such calls changes
neither speed nor the result. Children are where gathering pays:

```python
qs = ["Where is the retry budget set?", "Who calls select_parts?"]
answers = await asyncio.gather(*map(rlm.ask, qs))
```

The cell's 600 s ceiling covers the whole gather. Await everything a cell starts before the cell
ends: a call made by a task that outlives its cell is refused, and so is one made after an
interrupt.

## Long commands from a cell

`rlm.bash` starts a command host-side and returns a handle at once:

```
h = rlm.bash("cargo test --workspace 2>&1")
print(await h.tail())
```

The job has no timeout of its own and `await h` has no deadline, but the cell does (600 s). For
anything long, start it in one cell and collect it from a later one; the handle waits in the
namespace. Wrap a wait in `asyncio.wait_for` when the cell must return. Dropping the last
reference to the handle (reassigning `h`, `del h`) kills the job; do not count on a handle
surviving a restart.

## The kernel's walls

- On macOS the kernel, its `%%bash` and `!` lines, `subprocess`, and `rlm.bash` jobs all run
  contained, and a cell cannot ask to leave (an `rlm.bash` job gets only the grants bash already
  holds). A step that needs the network, an install, or a write outside the tree goes through the
  bash tool, which can ask.
- A `%%bash` or `!` cell whose command is not read-only stops for approval in auto mode; Python,
  and `rlm.bash` inside it, do not.
- The kernel's environment is fixed when it boots. A plugin installed later is on PATH for bash
  and `rlm.bash`, which build PATH per command, but not for `%%bash` or `subprocess` until the
  kernel restarts.

## What survives

- A checkpoint is written shortly after each cell that ends cleanly. A cell that raised or was
  aborted is not checkpointed, so names it bound exist only in the live kernel until the next
  clean cell.
- A restart revives the last checkpoint, minus what cannot be serialized (a live connection, a
  running job handle) and any variable over 16 MiB serialized. After a compaction those oversized
  variables are also removed from the live namespace, and the state note names them.
- Keep big data in a file in the tree (parquet, npy, JSON) with a function that reloads it, so a
  lost name costs one call.

## Recovery, by what the result says

| The result carries | Do |
|---|---|
| `[IPython kernel was restarted; N names lost: …. Define them again before use]` | rerun what defined exactly those names; the rest is back as of the last clean cell |
| `[IPython kernel was restarted; in-memory state was lost]` | rebuild from files; assume nothing survived |
| `[cell aborted]` | split the work: keep finished items in a dict so a rerun skips them, or move it to `rlm.bash` |
| a restart on the cell after an abort | the interrupted code ignored the interrupt (native code), so the kernel was replaced; same as a restart |
| a boot error, such as `Failed to initialize rlm runtime in the IPython kernel:` | the next cell boots again; bash works meanwhile, so keep going there |
| `` `x` is not installed in the kernel. … `` | do not retry `%pip`; try the project's own venv (`%%bash` with `uv run`), a stdlib route, or ask the user |
| `next: the sandbox refused … cannot leave it, so do not retry; …` | do it with the bash tool, which can ask, or tell the user what needs the network or the path |
| `[this output shows the sandbox refusing the network …]` | the cell "succeeded" on a refused request; the same, via bash |

The first cell of a session waits for the boot, which may build the venv; that wait does not
count toward the cell's 600 s, so a slow first cell is not a hang.

A `yi.tools` call that fails raises:

| Raised | Means | Do |
|---|---|---|
| `yi.tools.UnknownTool` | no such tool, or one a cell cannot call | for a misspelling read `.suggestions`; `yi.tools.excluded()` says why `ipython` and a root's `ask_user` are out |
| `yi.tools.InvalidInput` | the arguments failed the tool's schema, or `read` was given a `kernel://` address | `help(yi.tools.<name>)` shows its parameters; for `kernel://` see below |
| `yi.Refused` | a rule, the wall or the user said no | do not route around it, by bash or otherwise: report it, or ask |
| `yi.tools.ToolFailure` | the tool ran and failed | `.result.text` is the tool's own message; a stale tag means read again and rebuild the patch |
| `ToolFailure` with `kind == "cancelled"` | the cell had already ended or was interrupted | move the call inside a cell that awaits it |

A cell that raises halfway keeps what its calls did: the edits before the error landed, and the
cell's result lists them. Read that list before running the cell again, since a rerun repeats
every write.

Your own variables are already in scope. Another kernel's object is
`await rlm.fetch("kernel://<name>/<var>")`, never `yi.tools.read`.
