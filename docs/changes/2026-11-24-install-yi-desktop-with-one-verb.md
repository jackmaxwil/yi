---
issue: Closes #2361
shares: #2344 this adds an `install` and a `tools` recipe to desktop/justfile and `tools` to `check`, beside the recipes that pull request renames
---
`just install` in `desktop/` now packages Yi Desktop and puts it in `~/Applications` (`--to DIR` for elsewhere). It refuses while the installed app is running, and keeps the bundle it replaces under `target/package/previous`, so one step back is a copy. `just tools`, part of `just check`, proves each of those on scratch folders. Signing and notarising the bundle come with the dmg. #2361
