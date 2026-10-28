---
issue: Closes #1711
raise: tests +265, comments +1, crate cli +62, crate console +132, crate tui +106, crate types +12
decision: "Hybrid page jumps + streaming text" | "reduce visual movement giving user motion sickness" | reversible via tui.follow: slide
decision: "Yes, use Still" | "No I am only talking about the terminal text stream" | reversible via an explicit tui.follow setting
decision: "Yes, hold three rows" | "No I am only talking about the terminal text stream" | reversible via removing the bounded collapsed-card reservation
---
Terminal text keeps its existing character reveal. Follow levels select one-row sliding, half-page jumps, or a stationary view with a hidden-row marker. Running cards reserve bounded collapsed result space, and notebook updates preserve a reader's scroll position.
