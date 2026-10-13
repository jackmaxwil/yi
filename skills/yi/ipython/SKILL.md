---
name: ipython
description: >
  When the persistent kernel beats bash, where its walls are (sandbox,
  approval, an environment fixed at boot), how cells hand work to bash and
  to background commands, what a restart or a compaction keeps, and how to
  recover from a restarted kernel, a cell cut at its ceiling, a sandbox
  refusal or a missing import. Use before data work, a loop over results,
  a log too big to read, or a long command run from Python. Do NOT use for
  one command whose output you read once: that is bash.
covers: tool:ipython
---

# ipython

`ipython` → state in variables → files across the bash seam → jobs by handle → checkpoints at clean cells → recover by name

## When the kernel wins

- The second time you would carry output from one call into the next, it
  belongs in a variable. Counts, sums, set differences and tables are
  computed and printed; the answer quotes the printed value.
- A bash result cut to `[full output: path]`, or any log you will query
  twice: open the path in a cell, filter it, print only what decides.
- Bash and the kernel share the working directory and nothing else; a file
  in the tree is how a value crosses between them.

## The kernel's walls

- On macOS the kernel, its `%%bash` and `!` lines, `subprocess`, and
  `rlm.bash` jobs all run contained, and a cell cannot ask to leave (an
  `rlm.bash` job gets only the grants bash already holds). A step that needs
  the network, an install, or a write outside the tree goes through the bash
  tool, which can ask.
- A `%%bash` or `!` cell whose command is not read-only stops for approval
  in auto mode; Python, and `rlm.bash` inside it, do not.
- The kernel's environment is fixed when it boots. A plugin installed later
  is on PATH for bash and `rlm.bash`, which build PATH per command, but not
  for `%%bash` or `subprocess` until the kernel restarts.

## Long commands from a cell

`rlm.bash` starts a command host-side and returns a handle at once:

```
h = rlm.bash("cargo test --workspace 2>&1")
print(await h.tail())
```

The job has no timeout of its own and `await h` has no deadline, but the
cell does (600 s). For anything long, start it in one cell and collect it
from a later one; the handle waits in the namespace. Wrap a wait in
`asyncio.wait_for` when the cell must return. Dropping the last reference
to the handle (reassigning `h`, `del h`) kills the job; do not count on a
handle surviving a restart.

## What survives

- A checkpoint is written shortly after each cell that ends cleanly. A cell
  that raised or was aborted is not checkpointed, so names it bound exist
  only in the live kernel until the next clean cell.
- A restart revives the last checkpoint, minus what cannot be serialized (a
  live connection, a running job handle) and any variable over 16 MiB
  serialized. After a compaction those oversized variables are also removed
  from the live namespace, and the state note names them.
- Keep big data in a file in the tree (parquet, npy, JSON) with a function
  that reloads it, so a lost name costs one call.

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

The first cell of a session waits for the boot, which may build the venv;
that wait does not count toward the cell's 600 s, so a slow first cell is
not a hang.
