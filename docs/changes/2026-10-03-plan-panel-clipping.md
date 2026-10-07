---
issue: Closes #1038
raise: crate runtime +2, crate tui +48, tests +172, comments +1
---
The plan panel and the plan's confirmation prompt stop cutting text silently and repeating it (Closes #1038). A HUD row wider than the screen ran off the right edge mid-word; it now keeps what fits and ends in `…` at the edge. A todo blocked with its own label as the reason read the sentence twice; it now reads `(blocked on user)`. The approval prompt showed two wrapped rows of its reason and dropped the rest with no mark; the second row now ends in `…` when more was cut. The plan's `accepted_by_user` confirmation led with `at revision N (args sha256:…)`, so the two rows a person sees held the hash and not the todo; the todo and its reason now lead the line. The prompt's `Listening…` orb is the approval state by design and is unchanged.
