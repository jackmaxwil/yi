---
version: 0.1028.0
decisions: D585
issue: Closes #2140
raise: crate console +23, tests +90
decision: A session's accent follows the folder it was started in and its identicon creature stays seeded by its own id; a child wears its parent's folder hue; a row with no folder keeps a hue of its own | owner, 2026-11-11, asked whether sessions in one folder should look identical: "Folder colour, own creature" | seed the accent by the session name and the avatar placement by the id again; this supersedes only the hue half of the console session avatars row and of the badge colours D417 says the id hashes to
ledger~: console session avatars | and the initials tile every terminal draws under it wears the name's accent | and the initials tile every terminal draws under it wears the session's folder accent, which the mark shares; sessions started in one folder share that hue, a child wears its parent's, and a row with no folder keeps a hue of its own
---
Sessions started in the directory the sidebar already groups them by now share one accent, in the sidebar, the palette tile, the pane hue and the kitty mark; each session keeps its own creature, seeded by its id as before, so two sessions in one folder differ by shape and agree by colour. A kitty image carries its hue in its pixels, so a hue that changes under a live row (its folder text changing on a poll, or another folder scrolling into view) now transmits the image again instead of leaving the old colour.
