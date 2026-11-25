# Package

`just package` builds the program twice with the `dist` profile (release with `overflow-checks`): without `tooling` for the bundle, and with it (the headless flags) for the dry run, kept beside the bundle as `target/package/yi-desktop-tooling`, never inside it. It lays out `target/package/Yi Desktop.app` (bundle id `dev.yi.desktop`, version from the workspace, `assets/icon.icns` when it exists) and runs the dry-run check: the bundle's binary must not be the tooling build, and the tooling build, started from inside `Contents/MacOS`, must write a PNG with `--snap`. It stops there: unsigned, not notarised, not installed. The script prints what signing would need.

`just install` runs `just package`, then copies the bundle to `~/Applications` (`--to DIR` for elsewhere). It refuses while the installed app is running, and moves the bundle it replaces to `target/package/previous`. `python3 tools/package/install.py --selfcheck` (`just tools`) proves that on scratch folders.

`just budgets` judges `tests/budgets/budgets.json` (three rounds, the worst against the budget); `python3 tests/budgets/run.py --record` rewrites the measured numbers. `just receipts` measures the packaged app against a private `yi serve` and writes `docs/receipts.md`.
