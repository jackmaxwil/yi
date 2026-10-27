---
issue: Refs #1680
---

The review bot no longer contradicts or repeats itself across rounds. Each lens and refuter now reads the findings earlier rounds raised on the PR with the fix each suggested (newest 12, the cut named), and is told to drop a finding that asks to undo one. A finding the owner overrode stays closed while its line is unchanged: a fresh finding quoting the same text on the same path is dropped before any refuter and counted in the round's dropped line.
