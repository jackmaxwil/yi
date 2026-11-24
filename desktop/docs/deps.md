# Dependencies

Every dependency a crate manifest names; review holds a manifest to this table. Versions are declared once in the root `[workspace.dependencies]`; a crate manifest adds `{ workspace = true }` and features only. The manifests are frozen: a task that needs a dependency that is not here says so in its report and stops short of adding it.

| Crate | Version or path | Used by | Why |
|---|---|---|---|
| `rune` | the fork at the pinned revision (no `ogg-audio`, no `dbus`; `native-file-dialog` on) | yd-ui, yd-panes, yi-desktop | The UI framework: windows, layout, text, headless tooling. `native-file-dialog` gives `Tx::open_folder` and `Tx::open_file` the Mac's own panels, which a headless script answers with `file-dialog`. |
| `rfd` | 0.17.2, through Rune's `native-file-dialog` | rune | The Mac's open and folder panels. Measured: one crate more in the macOS graph (its `objc2` 0.6 family is already there), the dist binary 41,442,768 to 41,500,464 bytes (+57,696), nine lock entries that build only for Linux (the feature also turns on Rune's `dbus`; `cargo audit` lists three of them as allowed unmaintained warnings). Considered: an `NSOpenPanel` in `yd-mac`, which cannot sit behind Rune's dialog seam without patching Rune, so the native path would be a second one no headless test reaches. |
| `rune-kit` | the fork at the pinned revision (feature `markdown`) | yd-ui, yd-panes, yi-desktop | Rune's components: Markdown, `FileTree`, `DiffView`, syntax colour, tab strip, pop-ups. |
| `rune-luau` | the fork at the pinned revision | yd-panes, yi-desktop | Script panes. |
| `rune-notify` | the fork at the pinned revision | yi-desktop | System notifications. |
| `yi-types` | `../crates/types` | yd-core, yd-sources | Yi's wire and session types; in yd-sources, the browser's mouse and key input (`cdp::MouseInput`, `KeyInput`) that `yi-browser` takes. |
| `yi-browser` | `../crates/browser` | yd-sources | Yi's client-local browser crate: Chrome frames and input for the web pane. Adds no crate to the lock beyond itself. |
| `serde` | 1.0.229 (`derive`, `std`) | yd-core, yd-sources | Wire shapes, rows, theme files. Same version Rune's lock uses. |
| `serde_json` | 1.0.154 (`std`) | yd-core, yd-sources | The wire, fixtures, theme files, and the language-server and debug-adapter protocols, which are hand-written here instead of taken from a protocol crate. Yi turns on `preserve_order` through yi-types. |
| `thiserror` | 2.0.21 (`std`) | yd-core, yd-sources, yd-mac | Typed errors at crate boundaries. Same version Rune's lock uses. |
| `vt100` | 0.15 | yd-sources, yd-ui | Terminal escape-code parsing (the source) and the parsed screen the terminal surface draws (the UI). The crate Yi's console uses. |
| `rustix` | 0.38 (`fs`, `process`, `std`, `stdio`; yd-sources adds `pty`, `termios`) | yd-sources | The PTY and its window size. The crate Yi's console uses. |
| `base64` | 0.22 (`std`) | yd-sources | Decodes the browser's frames. Already in the lock through Rune. |
| `png` | 0.18 (no default features) | yd-ui | Decodes a browser frame to RGBA on the source's thread, into the one `Pixels` a screen draws. The version Rune itself depends on, so no crate enters the lock; Rune's own `ImageCache::load` decodes on a worker into a new image and cannot fill a screen's fixed `Pixels`. |
| `objc2` | 0.5.2 | yd-mac | The Objective-C runtime binding Rune itself links for AppKit. |
| `block2` | 0.5 | none | Declared for AppKit callbacks; the status items did not need it. |
| `objc2-foundation` | 0.2.2 | yd-mac | Foundation types for the status items; Rune's version. |
| `objc2-app-kit` | 0.2.2 | yd-mac | `NSStatusItem` and `NSMenu`; Rune's version. |
| `yd-core` | `crates/core` | yd-sources, yd-ui, yd-panes, yi-desktop | Model, reducer, wire client; no pixels. |
| `yd-sources` | `crates/sources` | yi-desktop | Feeds from outside the app; the only place a process is spawned. |
| `yd-ui` | `crates/ui` | yd-panes, yi-desktop | The window state, the pane contract and the engines on Rune. |
| `yd-panes` | `crates/panes` | yi-desktop | One module per pane. |
| `yd-mac` | `crates/mac` | yi-desktop | The one crate allowed `unsafe` (menu-bar status items). |

## The Rune pin

Rune comes from `https://github.com/jackmaxwil/rune` at one full revision, never a branch: a branch moves under the lock file. The four Rune crates and the five `[patch.crates-io]` entries share it, so moving the pin is one search and replace in `Cargo.toml`, then `cargo update -p rune` and `cargo deny check` (`deny.toml` takes git sources by `rev` only). A first build fetches Rune from GitHub and caches it under `~/.cargo/git`. `yi-types` and `yi-browser` are this repository's own crates, so they are always at the commit being built.

| Source | Revision | What it is |
|---|---|---|
| the Rune fork | `826baf804d14859388f4f24998786b62811c34fc` | branch `yi-desktop`: upstream `dev@4a2e4379` (2026-11-18) and the three patches of `docs/rune-patches.md` |

## What later waves use instead of a new dependency

| Need | Answer |
|---|---|
| Docker, launchd, git | The `docker`, `launchctl` and `git` commands, started in `yd-sources`, their JSON or text read with `serde_json` and `str`. |
| Language servers, debug adapters | Hand-written message types over `serde_json` on the child's stdio. |
| Watching theme and settings files | A worker thread that compares modification times. |
| PNG frames from the browser | `base64` in the source, then `png` in `yd_ui::screen::surface::Screen::push_png`, into `Pixels`. |
| An async runtime | None: threads and bounded channels. |

## What `just supply` reports in Rune's tree

`deny.toml` keeps every rule strict for this workspace and scopes the graph to the two macOS targets. Rune's own tree is excepted by name there: duplicate versions inside `rune`, `rune-kit`, `rune-luau` and `rune-notify` are skipped; Rune's path dependencies without a version are a warning; one git source is allowed (`mlua`, pinned to a revision by `rune-luau`). Turning off Rune's `ogg-audio` and `dbus` features keeps the MPL-2.0 audio codecs and the unmaintained `zbus` crates out of the graph. Upstream `4a2e4379` brought four crates through `rune-taffy`: `stacker` 0.1.25 (deep layouts continue on a grown stack), `psm` 0.1.32 (its build script compiles assembly), and `psm`'s build dependencies `ar_archive_writer` 0.5.5 and `object` 0.39.1; `ar_archive_writer`'s licence, `Apache-2.0 WITH LLVM-exception`, is allowed for that crate alone. The owner confirms these exceptions (see the plan's status file).
