---
issue: Closes #1901
raise: comments +2, crate runtime +30, tests +136
---

A spawned session now reads outside text through the screen its root holds. The root attaches the
screener at session open, and every child it spawns inherits the endpoint, the cut
(`classifier.screenAt`) and the cap (`classifier.screenMaxChars`) over its own journal: a flagged
page a child fetches carries `screen="flagged:0.93"` on its fence header, exactly as the same page
would in the root, and the `classify` row under consumer `inject` is written where the child will
be read rather than into the root's ledger.

The child gets its own breaker and its own per-text cache, so a child's dead endpoint pauses that
child's screen and nobody else's, and a text the child screened is not sent again by the child. A
root with no screener attaches none to its children, so a session whose classifier is unset still
marks nothing below it. Reader and worker children inherit the screen the same way, which arms the
screen a worker's untrusted attaches already pass through.

The seam is the one the broker and the provider ride: the host that spawns a child carries its own
session's screen, `ChildBuild` hands it to the factory, and the factory opens the child's slot with
a screener over the child's journal. A grandchild inherits from its parent the same way, since each
child's own host carries that child's screen.
