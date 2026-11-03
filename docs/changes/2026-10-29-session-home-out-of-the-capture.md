---
version: 0.834.0
issue: Closes #262
growth: +21 the home the store serves is derived from its own path and excluded with it; weighed: no new argument on `Checkpoints::open` for six call sites, and no size cap on the shadow repo that would leave the growth in place
raise: crate tools +21, tests +51, comments +2
---

A turn checkpoint captured the session's own home whenever the work tree held it. `yi --here` in a scratch directory whose `HOME` sat under that directory staged the home into `~/.yi/checkpoints` on every turn start and turn end, so anything under the home that changed between turns was stored again: measured on the unfixed binary with a home inside the work tree, 5 MB of churn under it added 5 MB to the shadow repo on every turn. A kernel venv rebuilt in that home is a few hundred megabytes, and eighty short requests filled a 48 GB tmpfs. A capture now leaves out the home the store serves when that home lies inside the work tree, which is where the runtime keys the store (`<home>/.yi/checkpoints`); in a session whose home is the work tree itself, `yi` started in `~`, that home cannot be spared whole without sparing the tree, so the capture spares `~/.yi` and the caches beside it (`~/.cache`, `~/.local`, `~/.cargo`) and keeps the rest of the tree; it costs undo coverage of `~/.yi` outside the store in that invocation, and a cache directory outside those three names is still captured.
