---
issue: Closes #1367
raise: crate console +93, crate runtime +9, crate types +9, tests +1
---
The settings pane now picks OpenRouter's providers in app. The Models tab has two rows, `routing.order` (providers tried first) and `routing.only` (providers allowed), each typed as comma-separated names and saved as the array OpenRouter's `provider` object takes, so a choice reaches the next session with no hand edit of config.json; the other `routing` keys still ride verbatim from the file. A list row the pane edits (the two routing rows) shows whole and is typed there; every other list (`skills.global`, `node.isolation`, `lanes.land`) still shows an entry count and points at config.json, so a value parked in a list stays put and an argument carrying a comma is never split. A JSON row such as `keys` still leaves the daemon as its count only, and an unset list or JSON row still shows unset.
