---
version: 0.841.0
decisions: D519
issue: Closes #1926
raise: crate console +9
decision: the autofixer tells a red base and a person's call apart from a failure, and settles a lone binary overrun itself | a week of its ledger held 25 failures of which 8 were main's own red test reported as flaky, 9 were calls no model could make, and 8 attempts at $8.80 sent a size no edit could reach to a model; the 45 KB role prefix it kept measuring was its own `TMPDIR` spelled through the checkout with `..` | restore the `RuntimeError` raises, remove the `base-red` and `handoff` verdicts and the `raise_binary` call in scripts/pr_autofix.py, and put the `..` back in autofix.yml
---
The autofixer's stops now say what stopped it. A PR whose failing tests also fail at its base gets a `base-red` comment and no label, and one issue names those tests, since nothing else reports that main is red. What no model can change gets a `handoff` comment naming the hour it tries again, also with no label. A lone overrun of the stored binary ceiling lands as its own `Ratchet:` commit without a model turn. `autofix:failed` now means a fix was tried and fell short.

A hidden sidebar keeps its way back on screen when a banner or a connection line takes the hint row: with no sessions the footer read only "no sessions for this root — alt+n starts one", and #1924's removal of the header label left nothing naming the chord. That row now ends in "⌥b sidebar", which also turns main's test gate green.
