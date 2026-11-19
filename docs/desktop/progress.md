# Yi Desktop: progress

How the app was built from a finished design to a tested app, 2026-11-15 to 2026-11-16, and what is true of it now. The design (a mockup of 24 frames and a 6,400-line design record) came first; nothing was built until it was settled.

## The method

Agents built it in waves of four to six, each agent in its own git worktree ("lane") with its own brief, one task each. A task was done when a script said so, not an opinion: `just task-gate <id>` (format, clippy with warnings denied, the crates' tests, the task's rendered scenes, an ownership check that the diff touched only the task's paths, an 800-line file ceiling, the dependency register) had to exit 0 before the agent committed. One cold reviewer read each wave against the design and the HAR rules (a HAR violation was a blocker), one fixer took what it found, and the lead merged each lane, re-ran the full gate by exit code and looked at every rendered scene beside its mockup frame.

Model by size: the hardest foundations (the keystone contracts, the wire client, the grid, the list engine, the code surface, windows, design mode) on the strongest model; features across several files on Opus; single modules on Sonnet; data over an engine on Haiku. Anything a script could decide was a script: fixtures and parity pulled from the mockup, ownership, integration, the HAR lint set.

## The waves

| Wave | What it built |
|---|---|
| 0 | The staging repository, the workspace on the pinned Rune fork, the gates and lane scripts |
| 1 | Five spikes, each kept as its task's base: a glass window and eight panes gliding inside the frame budget; one model across windows, tear-out and a menu-bar item; a terminal on Rune's grid with `vt100` and Yi's PTY; the code surface; the wire against a real `yi serve` |
| 2 | The keystone: the contracts every later task built on, every module stubbed and registered, the architecture and conventions documents, the ownership table |
| 3 | Fixtures and the parity checker pulled from the mockup; the scene and journey harness; the design system; the wire client and reducer; the grid maths with property tests; the row list engine (100,000 rows, one open) |
| 4 | The shell, the grid view with tabs and splitters, Chat, Files and Changes, the Terminal, the code surface and viewer |
| 5 | The command table, things and addresses with pick mode, panes as windows, full-screen chat and New chat, notifications and the menu bar, the first real-binary journeys |
| 6 | Agents, Trace, Logs, the findings model, Settings, Themes |
| 7 | Language-server and debug-adapter clients, the Findings hub, code marks, find references, the Debugger pane |
| 8 | The Web app and Report panes on Chrome, plugin panes in Luau, Docker and launchd feeds, list descriptions from fixtures |
| 9 | Design mode, the machines adapter and remote screen, empty, loading, offline and error states for every pane, the full shortcut map and accessibility labels |
| 10 | A parity sweep of every scene against its frame, the bundle and speed budgets as tests, journeys for every pane, a whole-repository HAR audit, the landing kit |
| 11 | Fixes from the journeys and the audit |

After every wave of panes the pieces passed alone but were not wired into the window; a single join task with the whole tree open to it wired them, and from wave 9 each pane task wired itself in.

## The dogfood rounds

The owner asked for a dogfood round with real spend: ten major journeys, real sessions, Sonnet testers, Sonnet or Opus fixers, no workflow. Each tester ran its own instance: a private home, a private `yi serve` with approvals on and a real model, fresh copies of three small projects with real bugs (a Rust CLI with two failing tests and a wanted flag, a Python package with a failing test, a static web page that lists an `undefined` row), and the real app window driven through macOS accessibility the way a person would use it.

The ten journeys: first run and orientation; fixing failing tests through approvals; quitting and coming back; reading code; the terminal; building a feature while watching agents, trace and logs; web development with pick and design mode; context and findings; layouts, windows, keyboard and settings; two projects at once.

| Round | What it found | Fixed by |
|---|---|---|
| 1 | No way to choose a project; the agent's edits landed in a Yi checkout the panes never followed; rust-analyzer never started and said nothing; the terminal could sit on "Starting the shell"; a prompt typed mid-turn was lost; Stop left an approval waiting; the Web app pane never showed a page | DF1 to DF6: a project picker (chip, ⌘O, a dropped folder, recent projects), panes that follow the thread's checkout with Changes diffed against its base and a local "Merge into main", a language server that starts or says why, prompts held until the turn ends, an honest Stop, a working web pane, attachments that reach the agent as text |
| 2 | Keyboard focus left on buttons and stray letters firing shortcuts; stale panes after a thread switch; no error marks; nothing naming which copy of the project a pane shows; a restart losing a waiting approval; Return answering an approval on another thread; Yi checkouts offered as projects | DF7 to DF11: focus where the person acted, error squiggles with a count, an "Agent's copy" line at the foot of Files, Changes and Code, waiting approvals kept across a crash, Return only on the card in view, shells kept per thread, hunk comments, narrow-width layouts |
| 3 | Every earlier blocker fixed; a normal task passed end to end. Left: a web tab attached from its menu sent the wrong text, a merge before the agent committed said only "nothing to merge", a hunk comment the agent did not recognise as one, overlaps at narrow widths | DF12 |

What the rounds taught the build: the idle CPU spikes the testers saw were the accessibility harness walking the window, not the app (it idles at about 1% of a core); a fixer whose lane was cut before a sibling merged rebuilt the sibling's fix a second way; several app instances at once made the harness drift between windows, so the final rounds ran one tester at a time.

## Numbers

Measured on an Apple M4 Max shared with other builds, so every time is a ceiling.

| What | Number |
|---|---|
| Rust in the six crates | about 101,000 lines, 1,059 tests, 195 rendered scenes |
| Binary, dist profile, in the unsigned bundle | 39.4 MiB |
| Start to first frame | 66 ms in the budget test (budget 160 ms); about 200 ms for the bundled app's real window |
| Frame with eight panes | 0.8 ms (budget 8.3 ms) |
| Scrolling 100,000 rows | 1.6 ms a frame |
| Opening a 2,000-turn thread | 293 ms (budget 825 ms) |
| Idle | no frames drawn and no commits while nothing changes; about 0.1% of a core |
| Supply | 453 crates in the lock file, 233 compiled for the Mac app; `cargo deny` and `cargo audit` pass with three allowed `unmaintained` warnings, all in Rune's Linux-only tree |

## Limits today

- The app is not in Yi: the tree, its decision row and its scope edit wait on the owner (#2257).
- On a live web page, pick and design mode take the whole page: Yi's browser link cannot yet ask a page about one element (#2278).
- A run whose approval was lost when `yi serve` itself restarted stays held, since no client can free it (#2263, #2280).
- Usage reads "no usage reported" for a model reached through a relay (#2191).
- The bundle is unsigned, and there is no mobile client ("i want mobile but later").
