---
version: 0.1067.0
issue: Closes #2179
cut: none scripts/checkout_guard.py
---
`checkout_guard check` no longer trusts the `made` mark alone. `made` now records the checkout's HEAD and uncommitted paths, and `check` refuses a script's discard when either differs, naming the new paths, so a person's edit made after the mark is not thrown away with the script's own. The gitleaks installer writes into a per-user 0700 directory under `$XDG_CACHE_HOME` (default `~/.cache/yi`) instead of a predictable `/tmp` path, stages with `mktemp`, refuses a directory other users can write, and re-checks the pinned sha256 on the installed binary before printing its path. The mirror publish script drops `PUBLISH_DIR` and reads only `TMPDIR`, which the release workflow now exports.
