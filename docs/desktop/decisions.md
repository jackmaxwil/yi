# Yi Desktop: decisions

What binds the app's build, in the owner's own words where they gave them, and what the lead decided under their standing rule "I choose, you veto". A decision here that the owner vetoes changes the code, and the change cites this file.

## The owner's answers

| Question | Answer |
|---|---|
| Where the code lives | "Inside Yi" |
| Where pieces Rune lacks get built | "In the app first" |
| Rune source | "i forked rune / use my fork as dependency / we will continue to pull upstream, but patch where needed" |
| Behaviour the mockup never drew | "I choose, you veto" |
| Scope | "keep this rust rune app lean", "focus on ui and ux", "i want mobile but later" |
| The Rust | "Verify HAR compliance throughout build" |
| The dogfood round | "Run through 10 major user journeys. Additionally, drive real sessions with real Yi spend and verify the app is actually usable for normal software engineering development." |

## How it lives in Yi

| Decision | Why |
|---|---|
| The app lands as `desktop/`, its own cargo workspace with its own lock file, toolchain and gate, never a member of Yi's workspace | Yi caps its dependency count; Rune alone locks several hundred crates |
| It is built in a staging repository and imported in one reviewed change | Yi's hook would run on hundreds of agent commits, and a structural change to Yi carries a decision row, a scope edit and a change file that belong in one landing the owner sees |
| Rune is a dependency on the owner's fork, pinned by revision; a patch is small, additive, one commit, and recorded | "patch where needed" while upstream pulls stay cheap. One patch so far: markdown paragraphs break a word too long for their line |
| v1 runs on today's server unchanged | Server changes go through Yi's strictest review; what the app wants from the server is filed as issues (below), and each pane works without its ask |
| The app talks to Docker, launchd, the debugger and language servers directly | Yi's own scope keeps these out of the server |
| Plugin panes are Luau scripts run inside the app | The only route where the machinery already exists |
| Findings are a view over what Yi already records, with triage kept by the app | The session is the one ledger; every other store is a view |

## Behaviour the mockup never drew

Taken by the lead; the owner may veto any.

| Area | Decision |
|---|---|
| Layouts | A menu on the top bar's layout icon and ⌥⌘1 to ⌥⌘5; when a pane leaves, its neighbours fill the space and no hole is ever left; minimum pane 160 by 120 points, minimum window 720 by 480 |
| Keys | One shortcut map generated from the command table; a chord belongs to one command; a key without a modifier outside a text field never runs a global command |
| Approvals | Return allows once, ⌘Return answers with Yi's first standing option (Yi orders them narrowest first and never offers the whole repository first), Esc denies; Return answers only a card the person can see |
| Stop | Answers the turn's waiting approvals as cancelled and holds queued prompts until the person sends them |
| Restart | A waiting approval is kept on disk so a crash or kill shows it again; a clean quit answers it cancelled; only the main window reopens, on the thread it showed |
| Projects | New chat starts on the folder the chip names (the shown thread's project, the folder the app was started in, the most recent project, else home); a Yi checkout is never offered as a project |
| The agent's checkout | When a thread works in its own Yi checkout, Files, Changes and Code show that copy and say so, with a button to the project folder; Changes diffs it against the branch it came from; "Merge into main" is a local `git merge --no-ff` that asks first, refuses when the project has uncommitted work, and is never a push |
| Shells | Each thread keeps its own shells while the app runs and starts them in its folder |
| Language servers | Start with build scripts and proc-macros off, since rust-analyzer would otherwise run a project's code outside Yi's sandbox |
| The web pane | Starts the Chrome Yi's config names (`browser.path`), as the console does; pick and design mode take the whole page on a live site until Yi can ask a page about one element |
| Windows | Settings and Themes open as their own 760 by 560 window; closing the main window with menu-bar items on keeps its thread running; only quitting cancels |
| Plugins | "Read your files" and "Run commands" are off by default; a script reaches the web only through the app's own fetch source for hosts the person granted |
| Left out of v1 | The Agents graph view, the plugin Browse and Updates tabs, the findings board, editing a command in an approval (the server cannot take one yet) |

## What the app asks of Yi's server

Each pane works without its ask and gets better with it. Filed on the forge; none is built.

| Area | Issue | Ask |
|---|---|---|
| Approvals | #2261 | Name the agent that asks in a permission request |
| Approvals | #2262 | Let a client answer a permission request with an edited command |
| Approvals | #2263 | List pending permission requests and re-send ones a client lost |
| Approvals | #2276 | Carry the title of the gate on the permission request it answers |
| Approvals | #2280 | Release a waiting permission request on cancel and store it as a stop |
| Threads and turns | #2264 | Add a request that renames a thread |
| Threads and turns | #2265 | Add a request that archives and restores a thread |
| Threads and turns | #2266 | Report a failed thread as its own state in the thread list |
| Threads and turns | #2267 | Push state changes for threads a client is not attached to |
| Threads and turns | #2273 | Add a title to each turn in the event stream |
| Threads and turns | #2274 | Mark an empty draft thread in the thread list |
| Threads and turns | #2277 | Put the parent session id on each session list row |
| Threads and turns | #2279 | Tell every client a session's working folder, not only those attached |
| Threads and turns | #2281 | Keep a prompt sent while a turn runs, and say what happened to it |
| Agents and events | #2260 | Stamp each session update with the time it happened |
| Agents and events | #2268 | Add parent, model, start time and budget to child updates |
| Agents and events | #2269 | Add pause, resume and more budget for a thread's agents |
| Agents and events | #2272 | Give agent events an Other case a newer server can use |
| Agents and events | #2275 | Mark whether a command that reaches a client source came from an agent |
| Findings, logs, settings, plugins | #2258 | Add a finding record a client and an agent can both write |
| Findings, logs, settings, plugins | #2259 | Serve Yi's own log lines to a client over the ACP socket |
| Findings, logs, settings, plugins | #2270 | Add a project scope to the settings the server serves |
| Findings, logs, settings, plugins | #2271 | Serve the plugin list and its state over the ACP socket |
| The browser | #2278 | Let a client ask the browser page any CDP method and read the answer |
| The browser | #2282 | Start Chrome with a mock keychain, free fds and its stderr kept |
| Usage | #2191 | Usage reads zero for a model reached through a relay (added to the existing issue) |
