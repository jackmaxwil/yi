# Memory

Host-written notes that outlive a session: one markdown file per fact, an index file, and a usage
counter per store. The model requests a write or a read through kernel verbs; the host validates,
renders and writes every file. The only part that reaches a prompt is the index, as one untrusted
yard block at session start. `§N` refers to [YI_DESIGN.md](YI_DESIGN.md).

Invariants:
- Only the root session keeps memory. A child session (depth > 0) gets no start block, and every
  verb refuses with `only the root session keeps memory`.
- At most 3 `memory.save` calls succeed per session. A refused save does not count against the cap.
- The host writes every note file in one canonical form: frontmatter `name`, `description`,
  `type`, then unknown keys as their raw lines, then the body.
- A save is refused if the text carries a yard fence (`<<<yi-external`) or a key-shaped secret
  (`PRIVATE KEY-----`, or an `sk-`, `ghp_`, `github_pat_` or `AKIA` run that starts at a token
  edge and holds a digit, so `disk-backed` is a word and not a key). Other
  refusals, each naming the field and bound: a missing or placeholder `description`, one over
  240 characters, a missing or unknown `type`, a body over 8 KiB.
- A note file that does not parse is never refused on disk. It is indexed by its first body line
  and counted as unparsed.
- Every write holds the store's `.lock` file lock and replaces files by staged rename.
- Each store keeps `ops.jsonl`, a journal of `save`, `edit`, `adopt`, `forget` and `read` records
  appended under that lock in the same critical section as the file change, each carrying
  `sha256(previous digest ‖ canonical record)` as the plan journal does. It names versions by
  hash and never holds a body; append order is the one order across lanes. An append reads only
  the last record for the previous digest, and the whole journal when the file does not end in one
  whole record; `yi memory rebuild` replays the whole chain and names a break.
- Every version's text is `objects/<sha256 hex>`. `memory.forget` deletes all of a note's objects,
  so no copy stays in the store. The save call's own arguments stay in the transcript of the
  session that made it, where `history.grep` still finds them.
- Reconcile at session start journals a note that predates the journal (`adopt`), a hand edit
  (`edit`) and a hand deletion (`forget` with `by: hand`, its objects deleted too).
- Each verb also appends a `custom{memory}` entry `{op, name, scope, hash}` to the root session.
  It never carries the body.
- Replies never carry a filesystem path; the kernel sandbox cannot read `~/.yi`.

Owner: [`crates/runtime/src/memory/`](../crates/runtime/src/memory/mod.rs) (`mod.rs` verbs,
`doc.rs` note format, `store.rs` files, `ext.rs` start block),
[`python/skills/memory/`](../python/skills/memory/src/memory/__init__.py) (kernel verbs),
[`crates/cli/src/memory.rs`](../crates/cli/src/memory.rs) (CLI).

State:
- `MemoryType { User, Feedback, Project, Reference }` and `Scope { Repo, Global }`
  ([`doc.rs`](../crates/runtime/src/memory/doc.rs)). `scope` selects the store at save time and
  defaults to `repo`. It is not written into the note.
- `usage.json`: `{ sessions, notes: { <name>: { saves, reads, last } } }`, where `last` is Unix
  seconds ([`store.rs`](../crates/runtime/src/memory/store.rs)).

Shapes (on disk):
- Repo store: `~/.yi/projects/<encoded canonical repo>/memory/`. The canonical repo is the parent
  of the git common dir, so every worktree and lane of one repository shares it (§14). Outside
  a repository the key is the canonical cwd.
- Global store: `~/.yi/memory/`. Each store holds `<name>.md` per note, `MEMORY.md` (the index, one
  `- [name](name.md) — description` line per note, hand-editable), `usage.json`, `.lock`, the
  journal `ops.jsonl` ([`MemoryRecord`](../crates/types/src/memory.rs)) and `objects/`.

## Verbs

The kernel imports the `memory` package at startup (§9). Each function is a host request.
- `memory.save(markdown, **fields)`: `fields` overlay the frontmatter. `name` is slugged to
  kebab-case (80 characters max; `memory` is reserved). Without a name, the first five words of
  the description are used. The same name updates the note and keeps its unknown keys. A key
  within two edits of a known one comes back as a warning. The reply is `name`, `description`,
  `type`, `scope`, `updated`, `warnings`.
- `memory.read(name, scope=None)`: matches a note by file name or slug. Failing that, it matches
  by index label or description, case-insensitive. It searches repo first, then global, and
  counts a read in `usage.json`. Failing both, it opens the top `memory.search` hit and warns
  `[no note has that name or hook · opened <name>, the closest of N by memory.search ·
  memory.search("…", limit=N) for the ranking]`; `forget` never does. The reply carries the body as `text`.
- `memory.search(query, limit=5, scope=None)`: ranks every note of both stores (or the given one)
  by BM25 (k1 1.2, b 0.75, Lucene's form) over its name, description and body, tokenized as
  lowercase ASCII words with `.`, `/` and `-` joining a path or identifier into one token and 61
  function words dropped. The reply is `hits` (`name`, `description`, `scope`), best first, and
  `total`; a cut list carries `[5 of N notes · limit 5 · memory.search("…", limit=N) for all]`.
- `memory.forget(name, scope=None)`: deletes the note, its index line and its usage entry.

## Claude Code

Claude Code keeps its own memories per working directory. `memory.importClaude` (on unless the
config sets it false) imports them at every session start, fresh or resumed, and
`yi memory import-claude` runs it by hand whether the switch is on or not.
- The directory is `~/.claude/projects/<the working directory, every byte but an alphanumeric a
  dash>/memory`; `~/.claude/memory` is the user-level one. The first lands in the repository
  store, the second in the global store. A directory that does not exist is not a failure and
  says nothing.
- Every note is copied once, its frontmatter tagged `source: claude` as its first key, so an
  imported note is told from a note Yi wrote. A note that carries no block gains one; a note
  whose block never closes still imports and is counted `unparsed`, as any other hand-written
  note. Yi never writes into `~/.claude`.
- Idempotent by content: the note file is compared byte for byte with the tagged source text, so
  a second start imports nothing and a changed source note updates its Yi memory. A note deleted
  at the source is kept and marked: its index line ends `⚠ stale`, and `yi doctor`'s
  `claude-memory` row counts what was found, imported and stale. The note is stale only when no
  Claude Code directory of this home still holds it, since one store serves every worktree of a
  repository.
- A note forgotten from the Yi store stays forgotten at later automatic starts while its Claude
  source remains. `yi memory import-claude` is the explicit act that adopts it again.
- The line a start commits reads `claude · N notes in ~/.claude/projects/…/memory · N imported ·
  N updated · N stale`, and an import that leaves the store past the block's load cap appends
  the `[… K of T notes loaded (LOAD_LINES 200); the rest: await memory.search("words")]` row.
  Counts and paths only: no note text reaches a log, a summary or a reply.

## Past sessions

`history.search(query, limit=8, offset=0)` (kernel `compact.search`) ranks user messages, assistant
text turns and compaction summaries by the same BM25, across every root session whose directory
under the sessions dir ran in this repository: the header's `cwd` resolves to the same canonical
repository (the git common dir), or sits inside it once its checkout is gone. Tool calls, their
arguments and their results are never indexed. A hit carries `session`, `entryId`, `type`, a
`snippet` in its text's own case (at most 160 characters; a cut one adds `cut: true` and a notice
row naming the cap) and `url`, `history://<session>/<entry>`, which `fetch` resolves across
lanes. A cut page names the call for the next one; a directory whose checkout is gone and whose
repository is therefore unknown is counted in a second notice line, never searched. The parsed
units are cached per file and re-read when a file's length changes. Root session only.

## Start block

`MemoryExt` is a built-in extension interested in `SessionStart` (§6). On every start, fresh or
resumed, it does the following:
1. Reconciles each store's `MEMORY.md`: drops lines whose note is gone or duplicated, appends a
   line per unindexed note, keeps every other line and the order.
2. Builds the block: the header (the verbs, the four types and the save template), the note
   counts, the repo lines, then `global:` and the global lines.
3. Loads at most 200 lines or 25,000 bytes per store, keeping the most recently used, in index
   order, then a row `[… K of T notes loaded (LOAD_LINES 200); the rest: await
   memory.search("words")]` that names `LOAD_BYTES 25000` instead when the bytes cut. With no
   notes, the block reads `No notes yet.`
4. Marks a line `⚠ <path>` when its text or description names a relative path that is missing
   under the git root, and `⚠ stale` for a note imported from Claude Code whose source note is
   gone.
5. Fits the block to the `memory` source budget (32,768 bytes) and emits it as
   `AttachExternal { source: "memory", trust: Untrusted }`, which lands in the yard (§6).

A fresh start also increments the repo store's `sessions`. The TUI commits the start summary
and one line per save or forget as footer cells; the HUD shows `saved N`.

## CLI

`yi memory [list | show <name> | search <words> | forget <name> | import <dir> | import-claude |
stats | check | rebuild]` works on both stores of the cwd; `search` prints every ranked hit.
`import` copies a directory of notes into the repo store untagged, skips identical
files, merges index lines and reconciles; `import-claude` is the section above, printing one
line per store. `stats` prints saves and reads per session per store.
`check` prints `path:line: reason` per unparsed note and exits 1 if there is any. `rebuild`
restores every note the journal holds and has not forgotten, recounts `usage.json` from the
journal, names a file that differs from its journal head and a version with no object, and
exits 1 on a missing object or a broken chain.

Settled by: D169, D277, D278, D279.
