---
issue: Closes #1711
raise: tests +258, comments +1, crate cli +62, crate console +132, crate tui +102, crate types +12
growth: +308 shared paint_pane and card seams carry the modes and bounded reservation; TuiOptions and AnswerRows moved to satisfy file caps instead of adding a scheduler or parallel follow state
decision: "Hybrid page jumps + streaming text" | "reduce visual movement giving user motion sickness" | reversible via tui.follow: slide
decision: "Yes, use Still" | "No I am only talking about the terminal text stream" | reversible via an explicit tui.follow setting
decision: "Yes, hold three rows" | "No I am only talking about the terminal text stream" | reversible via removing the bounded collapsed-card reservation
---
Terminal text keeps its existing character reveal. Follow levels select one-row sliding, half-page jumps, or a stationary view with a hidden-row marker. Ordinary running tool cards reserve bounded collapsed result space, child cards keep their bounded four-row answer window, and notebook updates preserve a reader's scroll position.

Fixed-clock frames measured `slide` as 22 one-row moves, `page` as two 15-row jumps, and `still` as five zero-move frames with its hidden-row marker. The live settings choice reaches visible and parked chats, while macOS Reduce Motion selects `still` only when config is unset.
