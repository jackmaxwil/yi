---
name: grep
description: >
  How to search so the hits become edits: which of grep, get_context, read
  and bash rg answers a question, how def, block, type and count narrow a
  sweep, how the tags on its rows feed edit, and what each capped or
  failed search tells you to do next. Use before any search, when a result
  ends in a [...] notice, or when a pattern errors. Do NOT use for a
  repository you have not oriented in (get_context first) or to read a
  whole file (read).
covers: tool:grep
---

# grep

`grep` → hits under `[path#TAG]` → `read` a window or `edit` the row; a notice names the call that gets the rest

## Pick the tool

- Where does X live, who mentions X: `grep`.
- What is in this file: `read`. One function of a big file: `grep def=true` for its line, or `read` with `find`.
- A repository you have not read this session: one `get_context`, then grep for the names it gives.
- Gitignored or binary files, or output you want to pipe: bash `rg`. `grep` skips what `.gitignore` excludes and always skips `.git`.
- A hit count is not a finding: a sweep over a tree counts tests and comments too. Read the matches before reporting a number.

## Narrow before you read

```
grep(pattern="fn parse_", type="rust", def=true)
grep(pattern=["TODO", "FIXME"], path="crates/tools", count=true)
grep(pattern="retry_budget", include="src/**/*.rs", block=true)
```

- `def=true` keeps declaration lines only: the cheapest map of where a name is defined. `count=true` ranks files by hits, and `files_with_matches=true` lists paths; both page 200 files.
- `block=true` shows the enclosing function of each hit instead of context lines, 20 hits a page. A line that opens no block reads `[line N opens no block; context shown instead]`.
- `type` is a fixed shorthand (rust, py, js, ts, md, toml, json, yaml, sh, html, css, c); another name answers `unknown type "…"; one of: …`. `include` is a glob; passing both is refused (`pass include or type, not both`).
- An array `pattern` matches any of its entries. `context` is clamped to 10 per side. `multiline=true` matches across lines and reports the first line.
- Plain text with regex characters: `literal=true`. `ignore_case` for case.

## Rows and tags

Hits print under `[path#TAG]` as `LINE:TEXT`; context rows are `LINE-TEXT`, and `--` separates gaps. Cite that tag and those line numbers in an edit with no read in between. Rows wider than 512 columns arrive clipped and never anchor.

- Only the first 20 files of a page get tags. Past them you see `[tags minted for the first 20 files of this page; the rows below name their file and anchor nothing — page with offset to tag them]`, and the rows read `path:LINE:TEXT`. Narrow the search, or `read` the file you mean to edit.
- `grep` over a document (PDF, Word, spreadsheet) searches the Markdown `read` shows, and its line numbers are read's. Documents past the per-search conversion limit are named as `not searched`; `read` one to convert it.
- A tag goes stale when the file changes. See the read skill for how edit rebases or rejects it.

## Paging and caps

- `[showing matches A-B of T — continue with offset=N]` is the next page. A total reading `at least N` means the collection stopped at 2000 hits: `[match collection stopped at 2000 before every file was scanned — narrow with path, include, or type]`. Narrow, do not page to the end.
- `[matches changed since the last page; offsets now count the current sweep]`: files moved under you (an edit, a build). Restart at offset 0 for a coherent list.
- `[offset N is beyond the K collected matches]`: you paged past the end.
- `[N binary files skipped (not text, not a document) — bash: rg -a for those]` and `[N paths left out: the reviewer wall's …]` are skips, not misses.
- `[the walk stopped after 200000 entries: name a narrower path]`, or a refused directory: give `path` a directory inside the working tree.

## Failures

- `invalid regex pattern: …` is a Rust-syntax regex error. Escape the character it names, or pass `literal=true`. Do not retry the same pattern.
- `path X does not exist (…)`: the `path` is wrong; list its parent.
- `No matches found` is a result. Before concluding the name is absent, drop `type`/`include`, try `ignore_case`, and remember gitignored files are not walked.

## Rewriting across files

`replace="new $1"` previews a diff per file and writes nothing; `apply=true` writes it. Read the preview, then apply. Past 50 files or 500 hits it refuses with `replace would touch …`: narrow with `path`, `include` or `type`. Non-UTF-8 files are listed as `skipped (not UTF-8)` and left alone. Apply prints diffs, not tags: `read` the lines you mean to edit next.
