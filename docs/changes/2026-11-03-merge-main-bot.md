---
issue: Closes #2015
---
The autofix sweep now runs after every push to main as well as on its schedule. Before any agent work on a behind PR, it tries a plain merge with main's baseline driver. A clean merge is pushed through the existing trusted path without a model call; a real conflict keeps the existing fixer path, whose brief names the conflicted files. `autofix:hold` still keeps the bot out, and a PR updated in the last five minutes is left alone so the bot does not race active work.
