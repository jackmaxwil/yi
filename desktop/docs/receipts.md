# Receipts

Yi Desktop 0.0.1, the unsigned bundle from `just package`, measured on 2026-11-15 on an Apple M4 Max that was shared with four other agents building and running their own lanes (load average 20 to 45 on 16 cores). Every time below is therefore a ceiling: a quiet machine is faster. Re-measure with `just package`, `just budgets`, `just receipts`.

| Receipt | Number |
|---|---|
| Binary size, shipped in the bundle (dist profile, no `tooling` feature) | 41,311,648 B (39.4 MiB) |
| Binary size, the dry run's `tooling` build beside the bundle (not shipped) | 43,501,264 B (41.5 MiB) |
| Bundle size | 41,312,567 B (39.4 MiB) |
| Cold start to first frame, real window, first launch after the bundle was built | 216 ms |
| Start to first frame, median of the next 4 launches | 200 ms |
| Idle CPU over ten seconds, one window open, nothing running | 0.10 % of one core |
| Memory at rest, real window, three seconds after the first frame | 111.8 MiB |
| Memory in the test process before the 100,000-row list | 111.9 MiB |
| Memory in the test process with the 100,000-row list drawn and scrolled | 404.8 MiB |

The bundle ships the dist build without `tooling`. Its sizes were measured after the bundle moved to it; the times and memory above are from the earlier bundle, which held the release `tooling` build, until `just receipts` runs again.

How: the bundle's own binary runs against a private `yi serve --model faux/faux-1` with a home under `target/receipts/home`; Rune's `RUNE_READY_DIR` file gives `ready_ms`, counted from process start to the first window being ready. The first launch is cold only as far as the disk cache allows (no `purge`). The 100,000-row memory is the in-process budget test (`just budgets`), whose process also holds the test harness, so the difference between the two test rows is the number to read.

## Budgets

| Budget | Number | Budget |
|---|---|---|
| start_to_first_frame_ms | 127.965 ms | 160 ms |
| frame_eight_panes_ms | 2.53 ms | 8.3 ms |
| scroll_100k_rows_ms | 1.671 ms | 8.3 ms |
| open_2000_turns_ms | 659.383 ms | 825 ms |
| type_in_composer_ms | 1.037 ms | 8.3 ms |
| idle_commits | 0.0 commits | 0 commits |
| idle_busy_frames | 0.0 frames | 0 frames |

## Supply

`just supply` exited 0. cargo deny: advisories, bans, licenses and sources ok; it warns that `yi-browser` names two path dependencies without a version (a wildcard, allowed for paths in `deny.toml`).
Crates in `Cargo.lock`: 453 (424 from crates.io, 2 from git (mlua, by Rune), 27 by path). Compiled for the Mac app: 233.

cargo audit allows 3 warnings, all `unmaintained`:

- `zbus_names` 4.3.5 (RUSTSEC-2026-0371): it is only reached through `accesskit_unix`, Rune's Linux accessibility bridge, which is not compiled on macOS (`cargo tree -i zvariant` shows it only with `--target all`). Nothing in this app can call it; the fix is Rune's to make.
- `zvariant` 5.15.1 (RUSTSEC-2026-0370): it is only reached through `accesskit_unix`, Rune's Linux accessibility bridge, which is not compiled on macOS (`cargo tree -i zvariant` shows it only with `--target all`). Nothing in this app can call it; the fix is Rune's to make.
- `zvariant_derive` 5.15.1 (RUSTSEC-2026-0372): it is only reached through `accesskit_unix`, Rune's Linux accessibility bridge, which is not compiled on macOS (`cargo tree -i zvariant` shows it only with `--target all`). Nothing in this app can call it; the fix is Rune's to make.
