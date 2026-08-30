<!-- Cold-reader narrative. Every section is prose a reviewer who was not in
     the session can follow. No checkboxes; gate proof is the CI checks tab,
     never pasted output. Delete a section only if it is truly empty
     (e.g. no UI change), and say so in Summary when it matters.
     `just pr-body` prefills the mechanical halves from the diff. -->

## Summary
<!-- What changed and why, in a few sentences a cold reader can follow
     without the diff open. -->

## User outcomes
<!-- What a user of yi can do, see, or rely on after this that they could
     not before. "Nothing user-visible" is a valid answer — say why the
     change exists anyway. -->

## UI changes
<!-- TUI/ACP-visible changes. For TUI: the headless frame dump or PTY
     evidence lives in the repo's test output; describe what moved. -->

## Files edited
<!-- A map, not a list: group by crate/area, one line each on why that
     area was touched. -->

## Schema changes
<!-- yi-types diffs, schemas.lock movement, fixtures added (never edited).
     "None" if none. -->

## LOC and justification
<!-- Net src LOC, measured. Over +150: this is the growth memo's home in
     PR form — what was weighed for deletion, why the bytes earn their
     place. -->

## Architecture notes
<!-- Version bump, changelog row, D-rows claimed or revised, feature-ledger
     rows touched (each names its journey test). -->

## Screenshots
<!-- Where a picture is the evidence (TUI frames, rendered output).
     "None" if none. -->
