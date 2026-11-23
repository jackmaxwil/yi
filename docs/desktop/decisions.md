# Yi Desktop: owner answers and server asks

The owner's answers and the server asks stay here. Yi-facing decisions are recorded in the canonical [decision log](../ARCHITECTURE.md#decision-log). `just adr <N>` renders each decision's ADR.

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

## The layout, decided 2026-11-21

The owner asked: "Panels should be session linked. Layout should be more dynamic. Too many blank panels are being opened with low user value." The design is `desktop/docs/layout.md`; its stages are #2304 to #2309. The owner's answers, each the option as worded:

| Question | Answer |
|---|---|
| Should a layout only set where things go, with panes appearing when there is something to show? | "Places only" |
| What should the app do without being asked when the agent does something? | "Add a quiet tab" |
| How much control over panes should the model get? | "Says what, not where" |
| Should pane sizes also change per session? | "Sizes stay with window" |
| When should a tab the app opened by itself go away? | "When its thing ends, if untouched" |
| What may the model's "front" do? | "Front only if you are idle there" |
| Where should panes about this Mac live? | "Stay with the window" |
| Should dragging a pane by its bar move the whole stack of tabs, or only the tab in front? | "Only the front tab" |

## What the app asks of Yi's server

Each pane works without its ask and gets better with it. Filed on the forge; none is built.

The server asks are the forge's issues #2258 to #2282 and #2191, one per row of the ask: approvals, threads and turns, agents and events, findings and logs, the browser and usage.
