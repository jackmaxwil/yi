---
issue: Closes #1335
raise: crate console +40, comments +2
---
The console rail's animated avatar stays on screen in kitty. The kitty graphics protocol deletes an image's placements whenever new data is transmitted under its id, and only a fresh placement shows the new data; Ghostty keeps the old placement instead. Each animation frame re-sent avatar 8000's pixels under the same id without placing it again, so kitty dropped the active session's avatar the moment it started to animate while Ghostty kept repainting it. Settling back to the identicon had the same gap, since the placement ledger still read the row as placed. Every re-transmit of an avatar that holds a placement now places it again at its recorded cell, inside the same synchronized update as the transmit, so kitty never shows a frame between the two.
