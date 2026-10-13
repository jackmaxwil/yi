---
issue: Closes #1367
raise: tests +1, crate console +46
---
The settings pane now picks OpenRouter's providers in app. The Models tab has two rows, `routing.order` (providers tried first) and `routing.only` (providers allowed), each typed as comma-separated names and saved as the array OpenRouter's `provider` object takes, so a choice reaches the next session with no hand edit of config.json; the other `routing` keys still ride verbatim from the file. A list row of names (also `skills.global`, `node.isolation`, `lanes.land`) is now shown whole and typed in the pane instead of showing an entry count and pointing at config.json; a JSON row such as `keys` still leaves the daemon as its count only.
