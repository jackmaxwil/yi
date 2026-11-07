---
name: forge-pr-shape
description: The forge PR body follows .github/PULL_REQUEST_TEMPLATE.md, opens as a WIP draft, and cites its issue.
type: reference
---
Every section is prose, no checkboxes, and `Closes #N` or `Refs #N` in the body.

**How to apply:** `just pr open` judges the title locally before the forge does, and `just pr ready` takes the WIP marker off.
