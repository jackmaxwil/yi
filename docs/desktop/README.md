# Yi Desktop

Yi Desktop is a native Mac app for Yi: a client of `yi serve`, written in Rust and drawn with Rune (the owner's fork). One window is a dense grid of panes around a conversation; any pane can become its own window, and the whole window can become one conversation. The app talks to Yi over the same ACP socket the console uses, and to this Mac's own tools (git, a shell, Chrome, rust-analyzer, LLDB, Docker, launchd) directly.

This folder documents the app while it is built outside this tree. The code is not in Yi yet: it lives in a staging repository and lands here as `desktop/`, its own cargo workspace, in one reviewed change (#2257).

| Document | What it holds |
|---|---|
| [progress.md](progress.md) | How the app was built, what each wave and each dogfood round found, and the measured numbers |
| [decisions.md](decisions.md) | The owner's answers, the canonical decision log, and what the app asks of Yi's server |

## Status, 2026-11-16

Built and tested; not landed, not signed, nothing pushed.

- Eleven build waves (53 planned tasks plus join and fix tasks) and twelve dogfood fixes are merged in the staging repository; its own gate (format, clippy with the HAR lint set, tests, 195 rendered scenes, mockup parity), the real-binary journeys and the supply check (`cargo deny`, `cargo audit`) pass.
- Three dogfood rounds drove the real app with a real model (Claude Sonnet 5.5) through ten everyday journeys. The last round found every earlier blocker fixed and a normal task working end to end: choose a project, ask, approve, review the diff, comment on a hunk, have the agent commit, merge into the project.
- A dry run of the landing (the tree copied in as `desktop/`, every offline Yi guardrail run over it) fails only `check_public_surface` and `check_scan`, both answered by the landing's own remedies.

## What a person can do in it

| Area | Panes and features |
|---|---|
| The conversation | Chat with streamed answers, folded turns, tool steps, an approval card with a queue ("1 of N"), Return to allow once, ⌘Return for Yi's narrowest standing yes; a composer with `@` files, attachments, model and effort; prompts typed mid-turn held and sent when the turn ends; Stop; full-screen Just chat; New chat on a chosen project (chip, ⌘O, or a dropped folder) |
| The code | Files, Changes (a diff against the branch the agent's checkout came from, revert and comment per hunk, "Merge into main" as a local merge that is never pushed), a code viewer with rust-analyzer hover, definitions, references and errors, a debugger over LLDB or GDB |
| Running things | A real terminal per thread (shells survive a thread switch), the agent's commands in a read-only tab, a Web app pane on Chrome with pick mode and a design mode, a Report pane for pages the agent writes |
| Watching the work | Agents, Trace, Logs, Findings (a view over what Yi records, with local triage) |
| This Mac | Containers (Docker), Daemons (launchd), VMs, Assets and a remote screen behind one machines adapter; all read-only in tests |
| The window | Five saved layouts and ⌥⌘1 to ⌥⌘5, panes pulled out to windows and docked back, a command box (⌘K) built from one command table, the keyboard help (⌘/), themes as JSON files with live reload, settings (the app's and Yi's through the server), a bell, a dock badge and a menu-bar item when a thread you are not looking at needs you |
| Plugins | Panes written as Luau scripts, reloaded live |

## How it is built

Six crates in their own workspace, never members of Yi's: Yi caps its dependency count, and Rune alone brings several hundred crates.

| Crate | Owns |
|---|---|
| `yd-core` | Everything with no pixels: the model and its one pure `step`, feeds, the grid maths, addresses, commands, tokens, settings, findings, the wire client and reducer |
| `yd-sources` | Things outside the app, each a feed on a worker thread; the only crate that starts a process |
| `yd-ui` | Window state and the engines on Rune: theme, chrome, grid view, row list, code, terminal and screen surfaces, menus, overlays |
| `yd-panes` | One module per pane; most panes are data over an engine |
| `yd-mac` | Menu-bar status items; the only place `unsafe` may appear |
| `yi-desktop` | The program: root view, shell, windows, notifications, menu bar |

The primitives are few and every pane is a composition of them: a model with one pure step, feeds with a status (loading, live, empty, offline, error), a grid that always tiles with no hole, a pane table, a row list engine, things with stable `yi://` addresses, one command table that drives buttons, menus, the command box and shortcuts, and design tokens. The staging repository's `docs/architecture.md` is the full map; it moves into `desktop/docs/` at landing.

The Rust follows the same high-assurance rules as Yi: `#![forbid(unsafe_code)]` outside `yd-mac`, clippy denying `unwrap`, `expect`, `panic`, `todo` and string slicing, typed errors, and a whole-repository HAR audit before the first landing.

## Running it today

From the staging repository, with an installed `yi` on `PATH`:

```
cargo run -p yi-desktop
```

The app finds a running `yi serve` or starts one, as the console does. `just check`, `just journey`, `just supply` and `just budgets` are its gates; `just package` makes an unsigned bundle.

## What is open

Open work is on the forge, never in this folder: the landing (#2257) and the server asks listed in [decisions.md](decisions.md).
