---
name: get-context
description: >
  How to use the orientation packet: when one call replaces a round of
  exploring, what each layer is for, which arguments sharpen it, and what to do
  when a layer is absent or cut. Use at the start of a task in a repository you
  have not read this session, and before touching unfamiliar code. Do NOT use
  to find a known symbol (grep), to read a file (read), or a second time in the
  same repository: the packet does not change as you work.
covers: tool:get_context
---

# get_context

`get_context` → read the header → follow the layer that names your target → read, grep, edit

## When

Call it once, first, in a repository you have not read, then go to the files it names.
Skip it when you already know the file (read it) or the exact name you want (grep).
Pass `task` in plain words and, if you can name one, `symbol`; both are optional but each
turns on a layer that is otherwise absent.

## Read the header first

The packet opens `COMPLETE` or `PARTIAL - k of n layers`. A missing layer is absent, not
empty: it prints `absent: <reason>` under its heading, and the reason is the recovery.

| Layer | Needs | Use it to |
|---|---|---|
| task map | `task`, ripwire installed | pick the first files and signatures to read |
| symbol neighborhood | `symbol`, ripwire installed | see callers and callees before changing it |
| file skeletons | source files | see declaration heads without opening files |
| git change heat | a git repository | find where work has been happening |
| gate commands | a `justfile`, `Makefile`, `Cargo.toml`, `package.json` or `pyproject.toml` | learn the check command |
| prior issues | `.yi/mining/issues.jsonl` | see recurring defects already mined |

`absent: no task argument was given` or `absent: no symbol argument was given` means call
again with that argument. `absent: ripwire binary not runnable: ...` means the map layers
need ripwire, installed with `yi plugin add ripwire`; the other four layers stand without
it. The task map is a guide: its rows name where to start, not what is correct.

## Following up

- Task map rows are `path:line  signature`; `read` the path. The neighbourhood matches
  calls by name, so a row can be a same-named function elsewhere; confirm with `grep`
  before trusting it. When it finds nothing either way, it says so and points to grep for
  uses of a type.
- Skeletons are ordered: files defining your `symbol`, then files mentioning it, then
  change heat, then name. Pass `symbol` to bring its file to the top.
- Take the gate line as the command to judge your change; run it with `bash`.

## Capped layers

Each layer is clamped, and each cut says what it dropped:

- `[skeletons truncated: N of M files at ...; read a directory for its skeletons, or pass symbol to rank its files first]`
- `[N of M heads, cap N per file — grep def=true for all]`: `grep` with `def=true` lists them.
- `[callers: 20 of N rows, cap 20 — bash: ripwire . --callers=... --json --limit=N]`: run the
  command it prints.
- `[task map: N of M signatures, cut at ripwire's byte budget — bash: ripwire . --for=... ]`:
  likewise.
- `[<layer> truncated at 4000 bytes]`: the layer was cut at its end; ask the narrower question.

Run the one call a notice gives instead of calling `get_context` again.
