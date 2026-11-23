# Yi Desktop layout: panes that belong to a session

**Status.** Design, decided with the owner on 2026-11-21. Built: stage 1 (#2304), stage 2 (#2305) and the first part of stage 3 (#2306: a slot holds a stack of tabs, a pane opens in the stack of its place, each session keeps its front tab). Not built: the rest of stage 3 (the one show action with its arbiter, preview tabs, the "+N" overflow, a pane's own thing and the two tables it retires) and stages 4 to 6 (#2307 to #2309).

**Supersedes.** The first plan's layout menu, tabs and split defaults, the pane switches in the Panes panel, the side table that binds a chat pane to a child agent, and the "half of the largest pane" rule in `architecture.md`, "The grid as built".

**Words.** The owner says "panel" and "session". The code says "pane" and "thread". They are the same two things; this document uses "pane" and "session".

## 1. Summary

The owner asked for this, verbatim:

> Yi desktop layout. Panels should be session linked. Layout should be more dynamic. Too many blank panels are being opened with low user value. Brainstorm and ideate and define a logic grouping dynamic layout intelligent system that supports both automatic determinstic auto layout as well as control by the LLM model and by the user with intuitive controls super mac native taking advantage of trackpad, keyboard, mouse, right click, two finger/three finger, hotkeys, click to drag mechanics <!-- codespell:ignore determinstic -->

The answer is five pieces. Every feature in the request is one of them or a combination of them.

| Piece | What it is | Status |
|---|---|---|
| Thing | What a pane shows: this file, this diff, this shell, this child agent, this page. A pane is a kind plus a thing. No thing, no pane. | ✓ `Thing` and `Address` exist; ✓ a pane carries one (`Pane.thing`), used so far by a child agent's chat |
| Session scope | Each session keeps its own panes. A pane may instead be kept by the window. | ✚ |
| Stack | A slot in the grid holds tabs, of any kind. One is in front. | ✚ (tabs exist only inside a pane today) |
| Show | The one action that puts a thing in front of the user. It says who asked (user, rule or model) and how strongly. | ✚ replaces 19 separate call sites |
| Rule | A row in a table: when this happens in the session, show that thing, there, until it ends. | ✚ |

The intelligence is the table and the model's own requests. Nothing is learned and nothing is guessed, so the same session always produces the same layout.

## 2. The problem, with evidence

1. **Layouts open empty panes.** The saved layout Everything opens eight panes (`crates/core/src/grid/saved.rs:140`). Three of them have nothing to show in a new session: Web app says "No page open", Report says "No page yet", Remote desktop needs a machine nobody named. Review, Screens and Code do the same on a smaller scale.
2. **The owner closes them by hand.** The owner's kept window state on 2026-11-21 holds one pane, chat, and still carries Everything's three row sizes (226, 190 and the rest). The layout was opened and the other seven panes were closed one at a time.
3. **The layout belongs to the window, not the session.** A window holds one layout (`crates/core/src/grid/windows.rs:48`) and shows one session (`crates/ui/src/shown.rs:55`). Switching sessions keeps every pane, whether or not the new session has anything for it.
4. **Session linking was patched in twice, each time by hand.** Terminal shells are swapped per session by a parking table (`crates/ui/src/shown.rs:66` to `:70`). A chat pane that shows a child agent is bound by a side table kept beside the grid (`crates/panes/src/chat/pane/child.rs:41`, `crates/app/src/windows/places.rs:15`). Both are the same idea, a pane tied to a thing in a session, written separately.
5. **Every opener has its own placement rule.** Nineteen places call `show_pane` (`crates/ui/src/grid_view/view.rs:408`), and each new pane takes half of the largest pane (`crates/core/src/grid/step.rs:193`). Opening four panes cuts the window into four tiles. Tiles are scarce; tabs are not, and the grid has no tabs across kinds.
6. **The model cannot point.** Yi has no way to say "look at this file". The app guesses from what a step names.

A case study from this session: the assistant writing this has a `show_pane` tool in its own app. It can show a diff or a file at a line; it cannot move, resize or close anything, and it is told what happened. That limit has never been the problem. Not being able to point at all would be.

## 3. Laws

The owner's standing words that bind this design:

- "matte, dense, flush, muted, cool, varied hues, little purple, no yellow or orange". Never spend vertical space on padding.
- "keep this rust rune app lean", "focus on ui and ux", "i want mobile but later".
- The session is the one ledger; every other store is a view. Triggers and routing read data. Caps are fuses, and a cap is named where it cuts.
- From the plan, kept: "No hole, ever". Minimum pane 160 by 120.

And from the two question rounds (section 11): layouts set places only; the app adds quiet tabs; the model says what, not where; sizes stay with the window.

## 4. The pieces

### 4.1 Thing ✓ and the pane that shows it ✚

`Thing { kind, label, address, excerpt }` (`crates/core/src/address/thing.rs:14`) and `Address` (`yi://<kind>/<stable id>[#part]`, `crates/core/src/address/uri.rs:29`) already name everything a person can attach. A pane today is only `{ id, kind }` (`crates/core/src/grid/windows.rs:20`).

```rust
pub struct Pane {
    pub id: PaneId,
    pub kind: PaneKind,
    pub thing: Address,        // ✚ what it shows
    pub scope: Scope,          // ✚ Session(ThreadId) | Window
    pub from: From,            // ✚ User | Rule | Model
    pub kept: bool,            // ✚ the user clicked in it, or chose Keep
}
```

A pane with no thing cannot be made. Chat is the one kind whose thing always exists: the session itself. Files (the session's folder), Logs and Trace always have a thing too. Changes, Web app, Report, Remote desktop, IPython, Agents, Design and Debugger have one only sometimes, and so exist only sometimes.

### 4.2 Session scope ✚

A window keeps the split lines (its tracks). Each session keeps which slots are filled and what is stacked in them. Switching sessions swaps the panes and leaves the lines where they are.

A pane with `Scope::Window` stays put across sessions. Containers, Daemons, VMs, Plugins and Language servers start that way, since they are about this Mac. Any pane can be moved to that scope by "Keep across sessions". One field covers both cases.

### 4.3 Stack ✚

A slot holds an ordered list of panes and names the one in front. A slot with one pane draws as it does today. The tab bar is the existing one (`crates/ui/src/chrome/tabs.rs:400`).

Four places, each a stack, each kind with a default:

| Place | Default kinds | Where |
|---|---|---|
| Chat | Chat | The side the owner chose (✓ `ChatSide`) |
| Work | Code, Changes, Web app, Report, Design, Remote desktop, IPython, a child agent's chat, Debugger, plugin panes | Beside chat |
| Side | Files, Agents, Findings, Assets, Containers, VMs, Daemons, Plugins | A narrow column |
| Shelf | Terminal, Logs, Trace, Language servers (its problem rows need the width) | A short row along the bottom |

A place is not a new kind of object. It is a slot in the existing grid plus one column in the kind table. A place with no panes takes no room, by the machinery that already closes the grid up round a pane that is out (✓ `leave`, `rejoin`, `crates/core/src/grid/leave.rs:180`, `:199`). The user can still split and dock freely (✓ `Drop`, `Beside`, `Side`); that makes more stacks.

### 4.4 Show ✚

```rust
pub struct Show {
    pub thing: Address,
    pub strength: Strength,    // Peek | Preview | Tab | Front | Beside | Window
    pub from: From,            // User | Rule | Model
}
```

One arbiter decides what a `Show` does:

1. A thing already shown is not shown twice. The existing tab is used. (✓ `Open` already "changes nothing" when asked again.)
2. **User**: does what was asked and takes focus.
3. **Rule**: adds a tab with a dot. It never changes the front tab and never moves focus.
4. **Model**: `Tab` and `Peek` behave as a rule's. `Front` brings the tab forward only if the user has not clicked or typed in that place for a short while (proposed: 10 seconds, to be tuned in use). Otherwise it becomes a dot, and chat shows a "Show me" button. Focus never moves.
5. Only the user splits, resizes, moves or closes. The one automatic tile is the first thing for a place that does not exist yet: the place opens at its layout size and chat keeps focus.
6. `Preview` is the single-click strength: one preview tab per stack, replaced by the next preview, kept by a double-click or an edit.
7. Tabs that do not fit collapse into a "+N" menu at the end of the bar. Nothing is dropped.

### 4.5 Rule ✚

A table the app reads. Each row: when, what to show, where, and when it ends.

| When (a fact in the session) | Show | Place | Ends when |
|---|---|---|---|
| The agent's first file edit | Changes, the session's diff | Work | The diff is empty |
| A child agent starts | Agents | Side | Every child has finished and been seen (as built: while the session has a child at all) |
| A command prints a local web address | Web app, that address | Work | The page is closed or the server is gone |
| A kernel cell returns a display | IPython, that kernel | Work | The kernel is gone (as built: a cell shows a picture, and the tab stays for the session) |

Four rows to start. An untouched tab a rule opened closes when its thing ends. A tab the user clicked or kept stays. Everything that was ever shown stays listed in the Environment column (✓ DF16), one click to bring back.

A right-click on any pane bar offers "Always open here" and "Never open by itself". Each writes a row to the settings file (✓ settings store). That is the whole of the user's control over the automatic part.

As built: both write one settings row, `panes.opens`, a line a kind. "Always open here" names the place of the slot the pane is in, opens the kind there in every session whether or not it has something to show, and covers kinds no rule names. Picking the ticked row takes the word back. The chat is offered neither.

## 5. The composition table

| Capability asked for | Composed from | Status |
|---|---|---|
| "Panels should be session linked" | Pane.scope | ✚ |
| No blank panes | Thing (no thing, no pane) | ✚ |
| A layout that sets places only | An absent pane is a pane that is out; showing it is `rejoin` | ✓ machinery, ✚ use |
| Own saved layouts | DF18's kept layouts, now read as places | ✓ |
| Deterministic automatic layout | Rule + Show + the kind table | ✚ |
| Control by the model | Show with `from: Model`, carried by one Yi tool | ✚ app, ✚ server |
| Control by the user | Show with `from: User` behind every click, key, drag and menu | ✚ |
| A child agent's chat pane | Chat × the child's address, in the work stack | absorbs ✓ DF17's table |
| Per-session terminals | Terminal × a shell, scope Session | absorbs ✓ the parking table |
| Open files as tabs | Code × a file, in a stack | absorbs ✓ the code pane's own tab table |
| Panes about this Mac | Pane.scope = Window | ✚ |
| Fill the window with one pane | ✓ the pane drawn alone (`grid_view::view::alone`) | ✓ |
| Pull a pane into a window | ✓ `Leave`, `Rejoin` | ✓ |
| A session as its own Space | ✓ app windows, plus macOS full screen | free |

## 6. Controls

Every row is a `Show` or an existing grid message. Nothing here needs a new idea.

| Input | Does | Built on |
|---|---|---|
| Click a thing (chat, Environment, Files) | Preview tab in its place | ✓ a file in Files (`preview_file`, the `preview` cell of the open-files table); ✚ the rest, with Show |
| Double-click | Keeps the tab | ✓ a file's row in Files; a press on the preview tab while it shows keeps it too |
| ⌘-click | Opens beside, as a split | ✓ a file, wherever one is pressed: a code pane about it right of the viewer (`GridMsg::Beside`); ✚ other things |
| ⌥⌘-click | Opens in its own window | ✓ a file: the same pane, shown as a tab and then `Leave`; ✚ other things |
| Space on the thing under the pointer | Peek: a card over the window, no pane; in a list, with the pointer on none, the row the arrow keys stand on | ✓ `overlay::peek`, the command `thing.peek` |
| Drag a thing onto a pane's edge or middle | Places it there. As built: a file dragged out of the Files pane; its name goes with the pointer and the place it would land is marked, as for a carried pane | ✓ `grid_view::carried`, `GridMsg::Put`, `Windows::zone_for` |
| Drag a tab to another stack | Moves it: a pane bar dropped on another pane's middle joins its tabs | ✓ `Drop`, `Spot::Onto` |
| Drag a tab out of the window | Makes a window | ✓ tear-out |
| Drag a pane bar | Moves the tab in front alone; the tabs behind it keep the slot. An edge of its own slot cuts it from them | ✓ `swap::land` |
| Pinch out on a pane | Fills the window with it | ✓ `GridMsg::Fill`, `grid_view::view::pinch` on `on_magnify` |
| Pinch in on a filled pane | Back to the grid | ✓ the same |
| Two-finger double-tap | Toggles fill | ✓ `on_smart_magnify` |
| Double-click a pane bar | Toggles fill | ✓ the bare part of the bar; a tab is pressed twice to keep a preview |
| Two-finger swipe sideways on a tab bar | Next or previous tab, one for each 48 points; coasting turns none | ✓ `grid_view::view::swipe` on `on_wheel` |
| Middle-click a tab | Closes it, when it has a cross | ✓ `on_middle_press` |
| Right-click a pane bar or a tab | Move to work, side or shelf, Split right, Split down, Pull out as a window, Keep, Keep across sessions, Close others, Always open here, Never open by itself, each on the pane under the pointer | ✓ `grid_view::aimed::carry`, `GridMsg::Place` |
| ⌥⌘0 | The Panes panel, now a list of the session's things by place, with the layouts. As built: a press on a row goes to its pane, a cross closes it, and the kinds with something to show and no pane are offered under "Not open" | ✓ `picker::places`, `Windows::listed` |
| ⌥⌘1 to ⌥⌘9 | Layouts, as places | ✓ |
| ⌘⇧↩ | Toggles fill for the focused pane | ✓ `pane.fill` |
| ⌘⇧[ and ⌘⇧] | Previous and next tab | ✓ |
| ⌃⇧⌘ arrows | Move the focused pane | ✓ |
| ⌘W | Closes the front tab | ✓ |

**Three-finger gestures: not building.** macOS owns three-finger swipes for Mission Control and Spaces, and three-finger drag is an accessibility setting that already works as an ordinary drag. An app that claims them breaks on most Macs. The Mac-native answer is that each session window can be a full-screen Space, which the system's own three-finger swipe then moves between.

## 7. The model's control

The owner chose "Says what, not where".

- **One tool, `show`** ✚, in Yi: an address and a strength (`peek`, `tab`, `front`). Nothing else. The model cannot read the layout, and cannot close, move or resize.
- **It reaches the app as a request the app answers** ✚, the way the protocol already lets an agent ask its client for a file or a terminal. The answer says what happened: brought to front, added as a tab because the user was busy there, or no display attached.
- **Offered only when a client says it can show things**, so the command line and the terminal app pay no prompt text for it.
- **It is an ordinary tool call in the session file.** A session opened on another machine can rebuild its quiet tabs from the record. The layout stays a view; the session stays the ledger.
- **The app works without it.** Stages 1 to 5 need no server change. The rules give the automatic half; the tool adds pointing.

Precedent, by name: Zed's "follow the agent" reads the locations a tool call reports. The deliberate difference: Yi's tool is a request, not a side effect, so the model learns whether the user saw it.

## 8. Precedents

| Precedent | Its idea | Here |
|---|---|---|
| Xcode Behaviors | A table of "when this happens, show that area" | Rule |
| Xcode's navigator, editor and debug areas | Fixed areas that hide when empty | The four places |
| VS Code editor groups and preview tabs | Tabs of any kind in a group; a single-click tab is replaced by the next | Stack, `Preview` |
| Kubernetes field ownership | A controller does not undo what a person set | `from` and `kept` |
| macOS Stage Manager | Each task has its own set of windows | Session scope |
| Safari | ⌘-click opens beside; pinch shows every tab | The click table; overview (deferred) |
| Finder Quick Look | Space to peek | `Peek` |

The one deliberate difference from an editor: panes come from what the session has, never from a global list of kinds to switch on.

## 9. What exists, what is new, what goes

**Exists and is reused:** the grid, its tiling checks and property tests; `leave` and `rejoin` (no hole); docking zones and `Drop`; tear-out windows; the tab bar; `Thing` and `Address`; the Environment column; kept layouts (DF18); the settings store; Rune's pinch, double-tap, wheel and middle-button events; a pane drawn alone.

**New:** four fields on `Pane`; the per-session seat list on a window; tabs across kinds in a slot; `Show` and its arbiter; the rule table and its four rows; the kind table's place column; the "+N" overflow; one Yi tool and one request on the wire.

**Deleted:**
- The 19 direct `show_pane` call sites become `Show`.
- The shell parking table and its owner map (`shown.rs:66` to `:70`).
- The child-chat side table and its kept file key (`child.rs:41`, `places.rs:15`).
- The code pane's own open-file tab table, and each pane's private tab handling where a stack now does it.
- The "half of the largest pane" rule (`step.rs:193`).
- The empty-state screens of every kind that can no longer exist without a thing. Loading, offline and error states stay.
- The per-kind switches in the Panes panel. As built: gone with stage 5's list by place.

The deletions are claims until stages 2 and 3 prove them; each stage reports the measured line count.

## 10. Build order

Each stage is a demo and a gate. The first needs no new idea and removes the worst pain.

| Stage | What | Demo | Gate |
|---|---|---|---|
| 1 | **No blank panes.** A layout seats only the panes that have a thing; the others are out until their thing appears. | Pick Everything in a new session: chat alone. The agent edits a file: Changes appears in Everything's own slot. | Every saved layout, on an empty session, draws chat only; the tiling property tests still pass. |
| 2 | **Panes per session.** Scope on a pane; switching sessions swaps panes, lines stay. Shells and child chats move onto it. | Two sessions with different panes; switch; quit and reopen. | A kept file from today's app still opens; a journey switches sessions and reads the panes. |
| 3 | **Stacks and Show.** Tabs across kinds; the arbiter; preview tabs; the dot. | Click three files: one preview tab. ⌘-click: beside. | A property test: no `Show` from a rule or the model ever moves focus, or changes the front tab of a stack touched in the idle window. |
| 4 | **Rules.** The four rows, auto-close, "Always open here", "Never open by itself". | A long offline session: tabs arrive with dots and leave when their thing ends. | Recorded sessions replayed through the reducer give the tab set the rules say, not what the app drew. As built: `a_recorded_session_ends_with_the_panes_the_rules_say`, six recordings. |
| 5 | **Mac controls.** Pinch, double-tap, tab swipe, Space, drag a thing to an edge, the right-click menu, the keys. | The owner drives it on a trackpad. | Real pointer events on the dogfood harness; every drawn key is in the key map. |
| 6 | **The model's `show`.** The Yi tool and the request. | The model shows a file at a line: front when idle, else a dot and "Show me". | An offline end-to-end test; the tool's text measured against the prompt budget. |
| 7 | **Overview.** Pinch in on the grid: every thing in the session as cards. | | Deferred until the first six are used. |

Stages 1 to 5 are app work, one issue each (#2304 to #2308). Stage 6 is a Yi server change (#2309).

## 11. Decisions, verbatim

Round 1:

| Question | Answer |
|---|---|
| "Should a layout only set where things go, with panes appearing when there is something to show?" | "Places only (Recommended)" |
| "When the agent does something on its own ... what should the app do without being asked?" | "Add a quiet tab (Recommended)" |
| "How much control over panes should the model itself get?" | "Says what, not where (Recommended)" |
| "Should pane sizes also change per session?" | "Sizes stay with window (Recommended)" |

Round 2:

| Question | Answer |
|---|---|
| "When should a tab the app opened by itself go away?" | "When its thing ends, if untouched (Recommended)" |
| "What may the model's 'front' do?" | "Front only if you are idle there (Recommended)" |
| "Where should those [Mac-wide panes] live?" | "Stay with the window (Recommended)" |

## 12. Verdicts on every idea considered

| Idea | Verdict |
|---|---|
| A pane is a kind plus a thing | KEEP |
| Panes per session | KEEP |
| Stacks | KEEP |
| One Show action | KEEP |
| Rule table | KEEP |
| The four places | MERGE: a column of the kind table |
| Preview tab | MERGE: a strength of Show |
| Auto-close | MERGE: a column of a rule |
| Keep across sessions | MERGE: the scope field |
| Layouts as places | FREE: an absent pane is a pane that is out |
| Own saved layouts | EXISTS (DF18) |
| A session as a Space | FREE |
| Pinch and double-tap to fill | KEEP |
| Tab swipe, Space to peek, drag a thing to an edge | KEEP |
| Overview on pinch-in | DEFER |
| Force-click to peek | DEFER: Rune reports no pressure; Space does the job |
| Two sessions side by side in one window | DEFER: two windows do it today |
| Follow the agent | CUT: the owner chose the quiet tab |
| Model reads or rearranges the layout | CUT: the owner chose "says what, not where" |
| Re-tiling the whole grid from a formula on every change | CUT: places and stacks are already deterministic, and the user's own docking survives |
| Learned placement | CUT: a table can be read; a model of habits cannot |
| Three-finger gestures | CUT: macOS owns them |
| Rotate gesture | CUT: nothing to rotate |
| Keep the newest few tabs | CUT: the owner chose end-of-thing; overflow is a "+N" menu |

## 13. Open questions

1. **The first thing for a place that is not there.** Proposed: the place opens at its layout size and chat narrows, with chat keeping focus. The alternative is a thin strip of tab names that opens on a click. The first is less to build; the owner should see it before it is settled.
2. **The idle window for "front".** Ten seconds is a guess. It is one constant, to be tuned by use.
3. **A child agent's own things.** Proposed: they open in the parent session's work stack, labelled with the child. A child is not a second session in the window.
4. **Files.** It always has a thing, so no rule needs to open it. Proposed: the user opens it and each session remembers.
5. **Today's kept window file.** Proposed: it is read as the arrangement of the session last shown; nothing is lost.
6. **The cost of the `show` tool** in prompt text, and whether Yi's one-in-one-out rule applies to a tool offered only to a client that asks for it. Measured in stage 6, decided by the owner.

## 14. Records owed

- Yi: a decision row for the `show` tool before its code (#2309).
- The app: `docs/architecture.md`, "The grid as built" and "The window, the shell and the panes, joined", rewritten with the stage that changes them.

## 15. Limits of this document

- It was written before any of it was built. Stage 1's own notes are in `architecture.md`.
- The count of 19 openers is a search for direct `show_pane(` calls outside tests.
- The claim that three structures are absorbed (shell parking, the child table, the code tab table) is a design claim. Stage 2 and stage 3 are where it is proved or dropped.
- Whether Rune's headless runner can replay a pinch was not checked; stage 5's gate may need a real trackpad for that one gesture.
