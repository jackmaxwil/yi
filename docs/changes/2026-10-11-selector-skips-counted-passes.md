---
issue: Closes #1190
raise: crate runtime +50, crate tools +9, comments +2
---
The line selector no longer adds back passing tests the reducer counted (Closes #1190). A `[N passing tests omitted: a-b]` row accounts for its lines, so the selector's hidden-line mask treats them as shown and never sends them; the reducer owns the row's wording (`yi_tools::reduce::passes_span`). On a 641-test `pytest -rA` run the selector had re-added 191 `PASSED …` lines, growing the result from about 5 KB to 14.5 KB.
