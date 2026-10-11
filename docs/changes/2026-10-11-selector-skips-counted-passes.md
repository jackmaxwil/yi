---
issue: Closes #1190
raise: crate runtime +43, comments +1
---
The line selector no longer adds back passing tests the reducer counts (Closes #1190). A line the reducer's own pass test (`yi_tools::reduce::is_pass`) calls a pass is never sent to the selector, whether or not its `[N passing tests omitted: a-b]` row survived the cut. On a 641-test `pytest -rA` run the selector had re-added 191 `PASSED …` lines, growing the result from about 5 KB to 14.5 KB.
