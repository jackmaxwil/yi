---
name: write
description: >
  When a whole file is the right unit of change, what the tag and syntax
  line a write prints are for, and how to recover when a write is refused
  or lands wrong. Use before creating a file, replacing one wholesale, or
  rewriting a line too wide to edit. Do NOT use to change part of an
  existing file (that is edit), to change many files by pattern (grep with
  replace), or to change a document, which nothing here writes into.
covers: tool:write
---

# write

`write` → a whole file → its tag and syntax verdict → edit for any change after

## Write or edit

- New file: `write`. Edit never creates one.
- Existing file, a few lines changed: `edit`. The patch is the diff, so nothing you keep is retyped and nothing is lost by a slip.
- Existing file, most of it changing, or a generated file you can reproduce whole: `write`.
- Same text in many files: `grep` with `replace`, previewed, then `apply`.
- Existing file you did not create or have not read this session: `read` it first. `write` overwrites without comparing, and what you did not read is what you destroy.

Send the complete file. `content` replaces everything, so a partial body leaves a partial file. Parent directories are created for you.

## What comes back

```
[src/greet.py#A1B2]
Wrote 214 bytes to /abs/path/src/greet.py
syntax: ok
```

- The header is a tag for the new content with every line counted as displayed. Edit at once by citing `[src/greet.py#A1B2]`, or with no tag at all, and do not `read` the file back to check.
- `syntax: ok`, `syntax: error line N: …` or `syntax: error: …` is a parse check for the languages that have one (JSON, Python, JavaScript, shell, Ruby, PHP, Go, Rust) when the checker is installed. No line, or `syntax: check timed out`, means no verdict, not a pass. An error line means the file is on disk and broken: fix that line with `edit`, not with another full write.
- A replacing write records a diff of what it dropped in the session record for the human reviewer; the model-facing result carries none, so you cannot check it. If the overwrite was not meant to drop content, `read` the file to see what is there now.

## Refusals

- `PATH is a symlink; refusing to write through it. Edit the target file directly.` Resolve the link and write the target path.
- `PATH is a binary document; read shows it as read-only Markdown …`, `… is a binary file …`, or `… is the read-only Markdown copy of a document`. A PDF, Word, spreadsheet or slide file cannot take text. Write a new file beside it, or change the original in the app that made it.
- `Denied by the reviewer wall: write targets PATH …`. The run fixed that path; report the mismatch, do not route around it with bash.
- `failed to create DIR: …` / `failed to write PATH: …` carries the operating-system reason, or the permission gate's own text for a protected location. Choose a path in the working tree.

## After a write

Later changes go through `edit` with the tag above. Another write to the same path mints a new tag; use the newest. What edit does when a tag is older than the file's current content, and which lines it may touch, is the patch guide's: `read("yi://tools/edit")`.

A line wider than 512 columns can never anchor an edit. When one must change, `write` the file, or use grep `replace` with `apply`.
