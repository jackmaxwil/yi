<!-- Cold-reader narrative. Every section is prose a reviewer who was not in
     the session can follow. No checkboxes; gate proof is the CI checks tab,
     never pasted output. Every section is required and read by the title
     job: replace each comment with prose, or with "None" and why. A comment
     left in a section fails the job. `just pr-body` prefills the mechanical
     halves from the diff, and `just pr open` opens the PR as a `WIP:` draft. -->

## Why needed
<!-- The ask this answers: `Closes #N` or `Refs #N`, the owner's words or the
     issue line quoted, and what stays broken or missing if it does not land.
     A reviewer tests the PR against this first: is it needed at all? -->

## Summary
<!-- What changed and why, in a few sentences a cold reader can follow
     without the diff open. -->

## User outcomes
<!-- What a user of yi can do, see, or rely on after this that they could
     not before. "Nothing user-visible" is a valid answer — say why the
     change exists anyway. Quote every user-facing sentence a test asserts,
     beside the sentences the user reads next to it: "completed without
     replying" was pinned by a test on the row above "Last answer: …". -->

## Seen red
<!-- One line per new or changed test: test name · the failure it produced
     against the unfixed code · where the fixture came from. This is the
     author's claim, not gate output — a test that was green against the
     unfixed code is rewritten, not shipped. "No tests changed" if none. -->

## UI changes
<!-- TUI/ACP-visible changes, and the frames or rendered output that show
     them: the headless frame dump or PTY evidence lives in the repo's test
     output; describe what moved. "None" if none. -->

## Files edited
<!-- A map, not a list: group by crate/area, one line each on why that
     area was touched. -->

## Schema changes
<!-- yi-types diffs, schemas.lock movement, fixtures added (never edited).
     "None" if none. -->

## Deleted / alternatives
<!-- Net src LOC, measured, and what the change removes. The simpler options
     weighed and why each lost. Over +150 this is the growth memo's home in
     PR form: what was weighed for deletion, why the bytes earn their place. -->

## Risk and rollback
<!-- What could regress, how you would notice, and how to revert. Name any
     durable data or schema a revert would have to carry back. -->

## Performance
<!-- The hot path touched and a measured number (command and result), or
     "No hot path touched" and why. -->

## Architecture notes
<!-- The change file under docs/changes/ (issue, growth memo, raises, decisions
     claimed or revised), feature-ledger rows touched (each names its journey test). -->
