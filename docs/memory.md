# Memory

Host-written notes that outlive a session: one markdown file per fact, an index file, and a usage
counter per store. The model requests a write or a read through kernel verbs; the host validates,
renders and writes every file. The only part that reaches a prompt is one untrusted yard block
ranked by the first typed prompt, with the bodies that prompt earns. `§N` refers to
[YI_DESIGN.md](YI_DESIGN.md).

Invariants:
- Only the root session keeps memory. A child session (depth > 0) gets no focused block, and every
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
`doc.rs` note format, `store.rs` files, `ext.rs` session start and focused block),
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

## Session start

`MemoryExt` is a built-in extension interested in `SessionStart` (§6). On every start, fresh or
resumed, it reconciles each store's `MEMORY.md`: drops lines whose note is gone or duplicated,
appends a line per unindexed note, keeps every other line and the order, and journals hand edits.
It emits the summary to the activity feed, and a fresh start increments the repo store's
`sessions`. It does not attach a memory block: the first typed prompt renders the only block a
request carries. A resumed session keeps a ranked block restored from its snapshot; one whose
snapshot has no memory block earns it at the first prompt, as a fresh session does.


## The first prompt

The first typed prompt decides which notes earn rows and bodies. The prompt reaches the host
before the first request, so `MemoryExt` renders and attaches the block once (`focused`); every
later prompt leaves the frozen prefix alone. The pass is a composition of what the store already
had:

1. Every note is scored by `memory::rank`'s BM25 over its name, hook and body, weighted by type:
   `feedback` 1.5, `project` 1.4, `user` 1.25, `reference` 1.0, untyped 1.1.
2. A note whose hook restates a higher-ranked note's hook (token Jaccard ≥ 0.7) keeps its index
   row but earns no body, so near-identical saves earn one body.
3. The bodies of at most 3 scored notes, each cut at 1,200 bytes, come first; the notes left
   over print as index lines in rank order, the unscored ones after them, at most 8.
4. The budget is the unranked index's row bytes, never under two body caps and never past 6,000,
   so one real body fits a small store; rows, then bodies, are dropped until the text fits it.
5. A cut names what it dropped and how to get it: `[… K of T notes (the index budget N, or FOCUS_ROWS 8); the rest: await
   memory.search("words")]`, a `[… body cut at 1200 B of N · the rest: await memory.read("name")]`
   row per truncated body, and `[no note shares a word with this request · …]` when nothing
   matched. A store that failed to load and a store past its load cap are named. A failed store
   also counts as one not-loaded entry in the focused cut's total.

A blank first prompt (a host or custom message carries no typed words) spends nothing and leaves
the first real prompt free to rank the block. A resumed session with a restored block never
re-ranks, so "continue" cannot replace the block the original task earned. A subagent's brief is
its first prompt and ranks the block like any other.

The eval that holds it is `evals/memory_inject.py` over `evals/fixtures/memory-inject`: eleven
real first messages, hand-labelled against a sixteen-note fixture store, scored on both arms
(`crates/runtime/tests/memory_inject.rs` renders this build's blocks and `replay` scores them
against `before/block.txt`, what the pre-change build rendered). The paid judge run is
`label --model openrouter/… --cap-usd 10`; it writes only the cases it judged to
`labels-judge.json`, leaves the hand-labelled fixture untouched, refuses uncountable costs, and
writes its ledger row separately.
The TUI commits the start summary and one line per save or forget as footer cells; the HUD shows
`saved N`.

## CLI

`yi memory [list | show <name> | search <words> | forget <name> | import <dir> | stats | check |
rebuild]` works on both stores of the cwd; `search` prints every ranked hit. `import` copies a directory of notes into the repo store, skips identical
files, merges index lines and reconciles. `stats` prints saves and reads per session per store.
`check` prints `path:line: reason` per unparsed note and exits 1 if there is any. `rebuild`
restores every note the journal holds and has not forgotten, recounts `usage.json` from the
journal, names a file that differs from its journal head and a version with no object, and
exits 1 on a missing object or a broken chain.

Settled by: D169, D277, D278, D279.
