# Tool ergonomics v3: fewer calls, nothing stale, everything editable

Landed: D117 (0.147.0).

```
status:  IMPLEMENTED 0.145.0 (2026-09-04), phases A–G; D117 is the record.
         File:line references describe the tree at 3bd0ce5 and are historical.
date:    2026-09-04
inputs:  the five-point brainstorm (read, edit, grep, todo, batching) · this
         tree at 3bd0ce5: crates/tools/src (builtins.rs, orient.rs, grep.rs,
         hashline/{snapshots,patcher,blocks,tool,prompt.md}),
         crates/runtime/src/tools.rs, crates/runtime/src/plan/tool.rs,
         crates/loop/src/run.rs, crates/types/src/plan/doc.rs · every
         ~/.yi session replayed for tool-call shapes · ~/Development/grid,
         timed against this worktree
ruling:  the model's seat decides. A feature that needs the model to learn
         a knob is out; a feature that removes a call, a re-read or a rule
         from the prompt is in. Grid stays a bash command plus one silent
         layer after edits. No new tools.
```

## 1. The seat

Written from the agent's chair. Five things I want, in the order I hit
them on a real task:

1. **Anything that shows me code, I can edit from.** Read, grep, bash
   `cat`, the edit response — all of it. I never re-read a file just to
   be allowed to change it.
2. **My numbers never go stale.** A line number from any earlier output
   in this session still works. I never re-read a file after editing it.
3. **One call gets the thing.** "Show me `render_section_result`" is one
   call, not grep then read. A big file shows me its shape and the part I
   asked for in the same answer.
4. **Blocks are exact.** `PUT N*` replaces the whole function in Rust and
   in Python.
5. **I fire everything at once, and I am told when I broke something.**
   Every read-only call in one message runs together. After an edit, the
   tool tells me if the file no longer parses or a caller no longer
   compiles. I do not have to ask.

The todo list is a sixth: a checklist I write as text, shown on screen.

Call counts for the four commonest loops, before and after:

| loop | today | after |
|---|---|---|
| find a symbol, change it | grep → read → edit → read | grep → edit |
| change a region of a big file | read → read → edit | read → edit |
| two edits to one file | edit → read → edit | edit → edit |
| look with bash, then change | bash → read → edit | bash → edit |
| see a definition and every reference to it | grep → read → grep | read |
| list a directory, pick files, read them | bash ls → glob → read ×N | read |
| rename a symbol across the tree | grep → edit ×N | grep → grep |

## 2. Ground truth (3bd0ce5)

Kept as is: `read` caps and `ranges`; `grep` regex/type/include/context/
paging with `[path#TAG]` headers; the edit response's renumbered ±3
windows; the snapshot store's four versions per path (snapshots.rs:75,
91); the loop's overlap of Read-kind runs (loop/src/run.rs:273); the
bash segment classifier (builtins.rs:261); the prefix-scan skeleton in
orient.rs:209; the `plan` tool and the plantree modal.

Sessions, all directories, 167 tool calls: bash 86 (33 of them `grid`),
ipython 35, read 27, plan 6, grep 2, edit 2. Max four calls in one
message. The model reads and searches through bash, whose output cannot
anchor an edit, and calls grid by hand.

Grid on this worktree: survey 245 ms warm, resolve 12 ms, scope 56 ms;
Rust and Python only; chart is pre-edit until the next survey.

## 3. Phases

Each phase is one version bump. A–D touch `tools` and `prompt.md` only.

| | what | removes |
|---|---|---|
| A | numbers never stale: line mapper, tag rebase, tag optional | the re-read after every edit; prompt rule 1 |
| B | everything editable: bash output tagged | the read after `cat`/`rg` |
| C | read and grep absorb the neighbours: shape by path, `find` with refs, grep modes | the grep→read pair; the read→read pair; the `glob` tool; `ls`; N edits for a rename |
| D | Python block resolver | the "resolver unavailable" reject |
| E | checklist todo + status-row count | twelve of thirteen plan ops from the model's path |
| F | read-only bash overlaps | serial `cat`/`rg`/`grid` |
| G | grid check after edit | the "did I break it" call |

### A. Numbers never stale

1. Line mapper in `snapshots.rs`: a line in an older stored version maps
   to the current one through the diff between the two texts.

   ```rust
   pub struct LineNo(NonZeroU64);
   pub enum Mapped { Same(LineNo), Moved { from: LineNo, to: LineNo }, Changed { from: LineNo } }
   pub fn map_lines(from: &Snapshot, to: &Snapshot, lines: &[LineNo]) -> Result<Vec<Mapped>, MapError>;
   ```

   Byte-identical lines map exactly. `Changed` is the only reject. No
   fuzzy case exists.
2. Rebase in the patcher where the stale tag is found today
   (patcher.rs:230): map every range and anchor; any `Changed` rejects
   with the existing mismatch context; otherwise rewrite to head numbers
   and apply. Header says `rebased #OLD→#NEW`.
3. Tag optional. `[path]` with no tag means the last snapshot this
   session rendered for that path — the store already knows it. With a
   tag, that version. Either way the section rebases to head. The
   model can still copy the tag; it no longer has to.
4. `prompt.md`: delete critical rule 1 and the "each edit renumbers"
   rule; add one line: numbers from any earlier output stay valid. The
   `<critical>` block keeps two rules: ranges tight, body is final
   content.

done: an edit with a two-versions-old tag after an insert above applies
with `rebased` in its header; the same after the anchored line changed
rejects and writes nothing; `[path]` with no tag applies; the request
prefix ratchet goes down.

### B. Everything editable

Bash tool, after execution: when the command is one segment classified
`read` or `search`, names exactly one existing file under the working
directory, and has no pipe or redirect, snapshot the file and prefix the
output with `[path#TAG]`. Otherwise leave the output alone.

```rust
enum Bridge { Tag { path: PathBuf }, Skip(SkipReason) }
enum SkipReason { MultiFile, Piped, Redirected, NotRepoFile, Mutating }
```

The bridge tags. It never renumbers or rewrites bash output. `cat -n`
and `sed -n` numbers line up with the file, so an edit anchors on them
directly; `rg -n` likewise.

done: `cat crates/tools/src/grep.rs` through bash anchors an edit with
no `read`; `cat a | head` does not; the decision lands in details.

### C. Read and grep absorb the neighbours

Two tools, one rule each: `read` answers for what a path names; `grep`
answers for what a pattern matches. Everything below is decided by the
shape of the input, not by a mode the model has to remember. Where a
knob is unavoidable it is one boolean with a plain name.

**read** — output is decided by what `path` names:

| path names | output |
|---|---|
| a file that fits the budget | the file, tagged (today) |
| a file over the budget | the window, then `[skeleton]` (≤40 rows) |
| a directory | listing: name, size, mtime, one skeleton line per source file |
| a glob (`*`, `?`, `**`) | every match; each file tagged; whole files while they fit the budget, skeletons after |

Plus `find="text"`: the window around the first literal match — the
enclosing block where a resolver exists (D), else ±20 lines — followed by
`[refs]`: every other line in the tree that mentions the matched
identifier, as tagged `path:LINE:TEXT` rows, capped at 20 with the
notice naming a `grep` for the rest. One call shows the definition, its
shape in the file, and who uses it. The identifier is the longest word
of `find` that is a word in the matched line; no language knowledge
needed. No hit: the notice lists up to five near misses by line.
`find` is exclusive with `offset`/`ranges`; on a glob it runs per file.

The `glob` tool is deleted in the same version: `read "**/*.rs"` is the
listing, and `path` on `grep` already scopes a search. One schema fewer
in every request.

**grep** — pattern is a regex by default; `literal: true` is the
exception (schema-lock bump). New modes, each one boolean:

| flag | effect |
|---|---|
| `block` | each hit renders as its enclosing block (D), not context lines; ≤20 hits, notice names `offset` |
| `def` | only hits on definition lines (the `DECL_HEADS` table in orient.rs) — "where is X defined" without guessing `fn`/`struct`/`def`/`class` |
| `count` | per-file counts only, sorted descending — size a change before reading |
| `multiline` | the pattern runs over the whole file text; a hit reports its first line |
| `replace` | see below |

`pattern` may be an array: hits match any entry, so "files touching X or
Y" is one call. `context`, `type`, `include`, `offset`,
`files_with_matches` and the tag headers stay as they are.

**replace.** `grep pattern replace="…"` renders the diff every hit would
produce, file by file, under the usual headers, and writes nothing.
`apply: true` on the same call writes it, snapshotting every touched file
the way `write` does, and the response is the same diff plus `applied N
files`. `$1`-style captures work because the pattern is a regex. Caps:
50 files, 500 hits; past either the call refuses before writing anything.
The tool's kind is `Read` unless `apply` is set, then `Write`, so an
applying grep never overlaps a read:

```rust
fn kind_for(&self, args: &Map<String, Value>) -> ToolKind   // default: self.kind()
```

added to the `Tool` trait beside `validate`, consulted by
`execution_mode` and by the permission preview. A rename across the tree
is two calls, the second one a copy of the first with `apply: true`.

done: `read crates/tools/src/hashline` lists the directory with one
skeleton line per file; `read "crates/tools/src/hashline/*.rs"` returns
every file tagged; `read path find="fn render_section_result"` returns
the function, the skeleton and its refs; `grep render_section_result
def:true` returns one hit; `grep old_name replace=new_name` renders a
diff and changes nothing, and with `apply:true` changes every file and
tags each; `glob` is gone from the schema set and the request prefix
ratchet goes down.

### D. Python blocks

`blocks.rs` gains `indent_block_resolver`; the patcher picks by
extension from a fixed table `enum Resolver { Brace, Indent, None }`.
`None` keeps today's message. `read find=`, grep `block` and `PUT N*`
all use the table.

done: `PUT N*` on a `.py` fixture replaces exactly the function.

### E. Checklist todo

The plan tool keeps every op it has; the model's path becomes one:

```
plan op=set list="- [ ] wire grep block\n  - [ ] resolver table\n- [>] rebase\n- [x] mapper"
```

`- [ ]` pending, `- [>]` running, `- [x]` done; nesting by two-space
indent. Parsed into `Todo` rows with a new `children: Vec<Todo>`
(`#[serde(default)]`), walked with an explicit stack. A label that
survives a `set` keeps its state and its delegation; a label that
disappears is dropped. `set` is the whole state change — no `start`,
`done` or `reorder` call is needed, though they still work.

Status row gains one span: `done/total · now: <first running label>`.
The plantree modal stays the full view.

done: a nested checklist round-trips through the plan doc golden
fixture; `set` twice with one `[x]` flipped moves the status row from
`0/3` to `1/3`.

### F. Read-only bash overlaps

`execution_mode` for `Exec` (runtime/src/tools.rs:155) asks one function
in `builtins.rs`: every segment `search`/`read`/`list_files`, no
redirect, no pipe outside those classes → `Parallel`; anything else
`Sequential`. `grid resolve|uses|scope|todo|roots` join the `search`
class in the classifier, so the 33 hand-typed grid calls overlap too.
No new tool.

done: a table test over twelve commands pins the decision; a loop test
with two read-only bash calls and one `write` asserts overlap and order.

### G. Grid check after edit

After a successful apply to a `.rs` or `.py` file, run
`grid check --quick` through the `command("grid")` helper in orient.rs,
two-second timeout, and append the first 40 lines under `[grid check]`.
Exit 3 is a finding and renders. Anything else renders
`[grid check: unavailable — reason]` and the edit still succeeds.

```rust
enum GridLayer { Clean, Findings(String), Unavailable(Unavailable) }
enum Unavailable { NoBinary, Timeout { ms: u64 }, Exit { code: i32 } }
```

The model never asks. A parse break or an uncompiled caller arrives in
the same turn as the edit that caused it.

done: an edit in a tree with no `.grid` renders the unavailable header
and applies; with `grid` off `PATH` the same; an edit that breaks a
caller renders the finding.

## 4. The surface after

```
read   path|dir|glob  [offset limit | ranges | find]
grep   pattern|[…]    [path type include context offset literal block def count multiline replace apply]
edit   patch          tag optional, numbers from any earlier output
plan   op=set list=…  plus the existing ops for delegation
bash   command        read-only commands run together and are tagged
```

Six tools become five. Every added flag is a boolean or a string with
one meaning; nothing in `read` is a mode.

Prompt text shrinks in `prompt.md` and grows by one line in the system
prompt: issue every independent read in one message.

## 5. Cut

- Delta read (`since=`): A makes the re-read unnecessary; the edit
  response already shows the hunks.
- A Read-kind `grid` tool: F's classifier entry gives the overlap with
  no new tool.
- `exclude` glob, `read paths=[]`, nested batch tool: knobs the model
  must learn, for gains `include`, the glob path and the loop already
  give.
- Grid callers under `read find=`: `[refs]` is a text search, tagged and
  never stale; grid's proven edges stay one bash call away.
- `word` flag on grep: `\\b` in a regex is shorter than a knob.
- Early-start execution while the message streams: invisible from the
  seat and the only loop change; measure the batch-size number from F
  first. Parked.
- Grid under read, datum-addressed edits, `grid edit apply`, per-line
  hash tags, a second todo tool, grid promises on todos, a grid daemon:
  see the previous revision's reasons; none removes a call from the
  seat.

## 6. Rules

- Every enum above is matched without a wildcard; every line number
  across a boundary is a `LineNo`; `get` and `checked_*` on anything the
  model authored.
- Every grid path degrades to today with a named header. Nothing outside
  G depends on grid.
- C and E are one schema-lock bump and one D-row each, numbered at PR
  time against open PRs. C's row also records the `glob` deletion and
  the `kind_for` trait addition.
- B, F and G record their enum in details so `yi stats` says whether
  the call count actually fell.
