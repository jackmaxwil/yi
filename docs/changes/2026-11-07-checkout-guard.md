---
issue: Closes #2080
---
A script no longer discards uncommitted work in a checkout it did not create. `scripts/checkout_guard.py check <dir> --by NAME` exits 1, naming the uncommitted paths, when `<dir>` holds work and NAME did not mark the checkout with `made`; a checkout handed to a person with `kept` loses its mark, and a directory that is no checkout of its own is a person's too. `just land` now refuses to fetch and merge `origin/main` into a checkout with uncommitted work. The land-fix and conflict-fix scripts outside the repo call `made` on the worktree they create, `kept` when they leave one for a person, and `check` before they remove one.
