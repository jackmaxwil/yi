---
issue: Closes #945
raise: tests +19, comments +1, crate tui +1
---
The status row cuts a long model name by its drawn width (Closes #945). `clip_cells` took the cut character by character against a width measured on the whole string, so a name with `⚠️` (1 cell alone, 2 in a string) kept one cell too many and the row ran a cell past its edge. It now grows the kept prefix and measures the string each time, as the sidebar's `pad_cells` does.
