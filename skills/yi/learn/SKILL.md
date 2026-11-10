---
name: learn
description: >
  /learn: turn this session into memories and at most three
  queued refine variants, each backed by a quoted line of the
  session. Writes memory and ~/.yi/refine/queue.jsonl only;
  never a prompt, rule or skill. `just refine round` judges the
  queue later.
---

# Learn from this session

`/learn [focus]` starts this turn. Its output is two kinds of record and nothing else.

**What goes where**

| Finding | Record | Writer |
|---|---|---|
| A fact about the user, the project or a working rule that should outlive the session | one memory note | `memory.save` (kernel), its frontmatter as the memory skill says |
| A change to Yi itself (a prompt sentence, a lever, a tool's code) that would have saved this session a request, a failure or tokens | one queue row | append a line to `~/.yi/refine/queue.jsonl` |
| Anything else | the reply | nothing |

**Steps**

1. Find the moments. `grep` with `path="history://self"` for failures, re-reads,
   repeated commands and the user's corrections; `read` each hit's address. The
   focus, when given, is where to look first.
2. Back every item with a quote. Copy the exact line from the `read` result. An
   item whose line you cannot quote is dropped and named in the reply as dropped,
   never saved from memory of the session.
3. Save memories. Check `memory.search` first: update a note that already says
   it rather than adding a second.
4. Queue at most three variants, one JSON object a line:
   `{"patch": "<unified diff against Yi's repository>", "target": "<what it should change, one line>", "quote": "<the backing line>", "entry": "history://self/<id>"}`.
   A variant with no patch (the session ran outside Yi's repository) keeps
   `"patch": ""`; `just refine round` skips it and a person reads it. The queue
   holds 64 rows: if it already does, write none and say so.
5. Reply with what was saved, what was queued and what was dropped, one line each.

**Recover**

- `memory.save` refused: its error names the field; fix the frontmatter and save again.
- The diff does not apply in `just refine round` (`patch_does_not_apply`): the
  base moved; the row stays queued until someone regenerates it.
- Never edit `crates/runtime/src/prompts`, `.ruler`, `skills/` or any rule file
  here: a variant only lands as a pull request the owner merges, after the round
  judges it better.
