---
issue: Closes #1711
growth: +428 shared follow paint seams and a macOS-only preference reader; builders and layout state were deduplicated instead of adding parallel follow state
decision: "Hybrid page jumps + streaming text" | "reduce visual movement giving user motion sickness" | reversible via tui.follow: slide
decision: "Yes, use Still" | "No I am only talking about the terminal text stream" | reversible via an explicit tui.follow setting
decision: "Yes, hold three rows" | "No I am only talking about the terminal text stream" | reversible via removing the bounded collapsed-card reservation
raise: crate cli +174, crate console +131, crate tools +1, crate tui +110, crate types +12, tests +319
---
Terminal text keeps its existing character reveal. Solo and console panes share the follow calculation: one-row sliding, half-page jumps, or a stationary view with a hidden-row marker. Ordinary running tool cards reserve bounded collapsed result space, child cards keep their bounded four-row answer window, and notebook updates preserve a reader's scroll position.

Fixed-clock frames measured `slide` as 22 one-row moves, `page` as a 15-row jump and a 5-row final step, and `still` as five zero-move frames with its hidden-row marker. The live settings choice reaches visible and parked chats, while macOS Reduce Motion selects `still` only when config is unset; its preference plist is read directly so the launch path spawns no helper process, and the reader is compiled for the macOS arm alone.
