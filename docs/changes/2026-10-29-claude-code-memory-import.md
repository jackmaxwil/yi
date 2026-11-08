---
issue: Closes #1859
growth: +1040 discovery, provenance and import share the existing Store and doc helpers; name listing and index construction were deduplicated, and the added tests cover the externally visible import and doctor behavior
raise: crate cli +31, crate runtime +995, crate types +11, tests +612, comments +18
decision: the Claude Code memories of the working directory and of `~/.claude/memory` import into the store that owns their scope at session start, behind `memory.importClaude` (default on) and tagged `source: claude` | the owner's memories were stranded in a shape Yi already reads, with only discovery, provenance and staleness missing, and a session start is the one moment a session reads memory | set `memory.importClaude` false, which stops the import and leaves the imported notes in place
ledger+: host-written memory (one file per fact, hook index) | the session start imports the Claude Code memories of the repository and of `~/.claude/memory` behind `memory.importClaude`, tagged `source: claude`, never writing into `~/.claude` and never over a note changed since it was imported (`cli_surfaces::yi_memory_import_claude_takes_the_working_directorys_notes`)
---

With `memory.importClaude` on, which is the default, every session start imports two directories before it reconciles the stores: the working directory's into the repository store, `~/.claude/memory` into the global one. Each note keeps its own file and gains `source: claude` as its first frontmatter key, so an imported note is told from a note Yi wrote; the index lines carry the Claude `MEMORY.md` wording where the source index had a line, and the store's own hook otherwise. A note that never closed its frontmatter imports too and is counted `unparsed` with the rest of the hand-written notes, and one that names an unreadable file no longer stops the batch: the count comes back as `unreadable` and the others land.

An automatic start does not resurrect a note whose Yi store journal ends in `forget`; the explicit
`yi memory import-claude` adopts it again. Source notes are listed and read once, so an unreadable
one is counted instead of vanishing, and Claude index lines meet the same secret and loop refusal
as note bodies. The session start reuses the index and notes the import reconciled, and the first prompt's block marks the stale ones.

The import is idempotent by content, not by timestamp: each note file is compared byte for byte with the tagged source text, so a second start imports nothing, a changed source note updates its Yi memory, and a note deleted at the source is kept and ends its index line `⚠ stale`. `yi memory import-claude` runs the same import by hand whether the switch is on or not, `yi doctor`'s `claude-memory` row counts found, imported and stale without writing, and an import that leaves a store past the block's load cap appends the load row that names the cap and the call for the rest. `~/.claude` is read only, never written, and no note text reaches a log, a summary or a reply: counts and paths only.

`yi memory import <dir>` keeps its own job and stays untagged, for any directory of notes; the import a session start runs is this one.
