---
issue: 2189
---
The forge has `priority:first`, `priority:next` and `priority:later` labels. The tracking hygiene check in `scripts/forge_tracking.py` now reports an issue that carries more than one of them, with the command that removes the extra labels, and the weekly run prints open prioritized issues with `priority:first` ahead of the rest. `.ruler/095-tracking.md` says what each label means.
