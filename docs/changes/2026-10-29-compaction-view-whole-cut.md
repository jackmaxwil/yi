---
issue: Closes #1506
raise: crate context +19, tests +127
---
A compaction view now compiles from everything the cut summarizes away, the prefix of a split turn included. One prompt followed by many tool turns used to compile an empty view, because the view stopped at the turn's start and the whole turn is the split prefix. An `ipython` call is briefed as `ipython <first non-blank code line>` instead of the bare tool name. A bash command in a brief is cut at `BRIEF_LINE_CHARS` with a `[… K of T chars (BRIEF_LINE_CHARS 160); the rest: read history://self]` row, as are the other brief fields. The read and modified file lists in a summary stop at `FILE_LIST_CAP` (100) files and end in a row naming kept of total, the cap and the `history://self` grep that finds the rest. The two summarization prompts nothing called (`SUMMARIZATION_SYSTEM_PROMPT`, `TURN_PREFIX_SUMMARIZATION_PROMPT`) are deleted.
