---
name: search
description: >
  Read what a parent or child said, past sessions, cut output. Use before
  re-running a command whose output is gone, when a child's task leans on
  its parent's work, when a parent needs its children's findings, or when
  the task says earlier, last time or we decided. Covers this session's
  log, the parent and child transcripts, this repository's past sessions,
  saved notes and MCP tools. Do NOT use to find code: grep the tree.
covers: tool:grep
---

# Search beyond the tree

Re-derive what can be re-derived. A transcript quotes a file as it was when someone read it;
the tree has it as it is. Go to history for what nobody can re-derive: what a person said, what
an agent decided or found, and output that is no longer on screen.

| The fact | Where it was written | The call |
|---|---|---|
| Code, a file's text | the tree | `grep`, then `read` |
| Output this session saw, now reclaimed or compacted | this session's log | `grep` with `path="history://self"`, then `read` the hit's address |
| What the parent asked for, decided or found | the parent's transcript | `read("history://parent/since/0")`; one line of its output: `grep` with `path="history://parent"` |
| What a child did or found | the child's transcript | `read("history://<child name>/since/0")`, or `rlm.result` once it ended |
| What another session found or was told, in any repository | its transcript | `read("history://<its name or id>/tail/40")`; `history://<root>/<child>` for its child |
| Whether this repository met the problem before | past root sessions | `await compact.search("words")`, then `read(hit["url"])` |
| A line an earlier session's command printed | past root sessions' tool output | `grep` with `path="history://repo"` |
| A saved note | notes | `await memory.search("words")` |
| An MCP tool | the MCP snapshot | `yi mcp grep "words"` in bash, when MCP is on |

`compact.search` and `memory.search` answer the root session only; a child asks its parent
for them. `compact.search` indexes what people and agents wrote, not tool output; `grep` on a `history://`
root matches every line, tool output included, and each hit's address is a `read`.

## Patterns

**Start from the parent.** A child whose task leans on earlier work reads its parent's
transcript before walking the tree. `since/0` is every entry as one line, `#seq` first and long
text cut; `tail/20` is the last twenty. For one entry whole, read the plain listing at that
entry: `read("history://parent", offset=<seq>, limit=1)`. A refusal that names a wall is the
design, not a fault: a walled reader works from its task alone, which keeps it independent.

**Brief a reader.** A reader child (the default role) gets a short prompt, not this doctrine, so
it does not know the family addresses. Put the fact in its question, name the address there
(`read history://parent/since/0`), or inline it with `partition=["history://self/<entryId>"]`.
Give addresses, not a retelling: `<entryId>` for one result, `tail/N` or `since/<seq>` for a
stretch, `custom/<type>` for one kind of record.

**Read before you ask.** Another session's transcript answers what it found or was told without
waking it. Read it by the name you know it by; a name several sessions share is refused with
their `history://<id>` addresses, so pick one and read again. A walled reader reads by id only.

**Collect the children.** A parent that needs what a child found reads that child's transcript
instead of asking it to say it again, and only the entries it needs.

**Recall before re-running.** A `[reclaimed: …]` line or a summary citing `(#entryId)` means
the output is still in the log. Recall it and read the entry; a re-run costs a turn and may
not reproduce it.

**Ask the repository's history** when the task names an earlier decision or a bug that feels
familiar: rank past sessions by the task's own nouns, open the best hit, and verify what it
says against the tree before acting on it.

**Rewrite an empty search.** One phrasing that misses proves little. Try two or three at once,
as regex alternation in `grep` (`retry|backoff|reconnect`) or as several words to
`compact.search`, which ranks any of them, before concluding something is absent.
