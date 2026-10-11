---
name: read
description: >
  How to read so the next edit lands: which tool to open a file, a
  directory, a document or a yi:// address with, how to window a large
  file, what the tags in a read mean once the file moves, and what each
  capped or refused read tells you to do next. Use before reading anything
  larger than a screen, when a read ends in a [...] notice, or when an edit
  complains about a tag or a line you did not see. Do NOT use to find where
  something lives (that is grep) or to orient in an unread repository (that
  is get_context).
covers: tool:read
---

# read

`read` → one window → its tags → edit; a notice names the call that gets the rest

## Pick the tool

- Know the file: `read` it. Do not `cat`, `head`, `sed` or `tail` it in bash; only `read` prints the `[path#TAG]` header edit anchors to.
- Know a name, not a file: `grep` first, then read the window it points at.
- Unread repository: one `get_context`, then read the entry points it names.
- Want one function of a big file: `read` with `find="fn name"`. It shows the enclosing block, and for a definition lists its references in files of the same language.

## Windows

Read one wide window, not many narrow ones: the byte budget is 50 KiB by default and grows with an explicit `limit`, so a deliberate big read is still bounded. Pick the window up front.

```
read(path="src/lib.rs", ranges=[[10,40],[90,120]])
read(path="src/lib.rs", offset=400, limit=300)
```

- `ranges` and `offset`/`limit` never combine, and neither combines with `find`. Each refusal names the pair ("pass ranges or offset/limit, not both").
- A cut reads `[showing lines A-B of T — continue with offset=N]` or `[byte budget … reached at line L of T — continue with offset=L]`. Pass that offset. A capped read also ends with the file's skeleton (a heading outline for Markdown): use its line numbers for `ranges`.
- A `find` that misses lists the nearest lines (`no line of PATH contains "…"; nearest by "…"`); retry with a word from one of them.
- A directory lists entries and source skeletons; a glob (`src/**/*.rs`) prints each match whole until 50 KiB, then lists the rest as skeletons. `[first 200 files matching …]` and `[showing 200 of N entries …]` mean: narrow the path.
- Rows over 512 columns arrive clipped and never anchor an edit. The notice prints the exact `bash: sed -n 'Np' path` that shows them.

## Tags and the next edit

Each read ends in a tag; edit cites `[path#TAG]` and the `LINE:` numbers beside it. An edit with no tag uses the latest one shown for that path.

- edit's rules on which lines it may touch and what it does when the file moved since your tag are the patch guide's: `read("yi://tools/edit")`. A refusal quotes the lines to read or re-issue with.
- `No version of PATH is on record for this session` means the tag was never shown or fell out of the short per-path history. Read the path again.
- `read` accepts a pasted `path#TAG`. If that tag is old it says `[path#TAG is not the current version: the file is now #LIVE; this is the current content]`: use #LIVE.

## Documents, images, binaries, encodings

- PDF, Word, spreadsheet, slides and similar files read as Markdown under a note that they are read-only; edit and write refuse the original. `pages="3-5,9"` picks PDF pages (`pages= applies to a PDF` otherwise). The first look is short and leads with an outline: read it, then `ranges`. Line numbers are those of the Markdown, and `grep` searches the same text.
- A spreadsheet with several sheets starts with `[sheets: name RxC, …]`; for analysis, load it with pandas in `ipython` rather than paging rows.
- `the kernel venv that converts documents is not built yet` clears on retry; do not work around it.
- An image refuses with the exact `attach_image` call to run in `ipython`. A NUL-byte file is `a binary file`: use `bash` (`file` names it).
- A UTF-16 or Latin-1 file reads decoded, under `[decoded from …; the file is not UTF-8, so edit cannot anchor to it]`. Change it with `write`.

## Addresses

A path containing `scheme://` is fetched, not opened. `read("yi://skills/<name>")` loads a skill, `yi://skills/<name>/<file>` a file beside it, `yi://tools/edit` the patch guide (`plan`, `todo` and `ask_user` have guides too; any other tool name answers with the list). `offset`/`limit` page only `local://`, `history://` and `kernel://`, and count bytes, entries and characters there; the cut reads `[more from offset N; …]`. An address returns text with no tag, so it never anchors an edit: read the file by path for that. `https://` and other external schemes refuse as `a tool call under permission, not a free read`.

## Failures

- `failed to read PATH: …` with a missing path: list the parent directory, or glob. Do not retry the same path.
- `no file matches "…"`: the glob matched nothing under the working directory.
- `[offset N is beyond end of file (T lines)]`: you windowed past the end; the file is shorter than your last read said.
