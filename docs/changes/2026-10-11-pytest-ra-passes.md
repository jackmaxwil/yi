---
issue: Closes #1186
raise: tests +22
---
The reducer now collapses the passes a `pytest -rA` short summary lists (Closes #1186). That summary names each pass as `PASSED path::test`, with no leading space, so the pass detector, which looked for the `-v` form ` PASSED`, let about 50 of them through on a 641-test run before a plain omitted-lines row. They now fold into one `[N passing tests omitted: a-b]` row like the `-v` passes.
