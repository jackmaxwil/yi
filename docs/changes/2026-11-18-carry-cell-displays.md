---
issue: Closes #1915, Closes #2288
growth: +170 the display reducer (kernel 155, one function absorbing a bundle, one building the cell's notices), the two result fields (types 13) and the result's details and notices (tools 2); weighed for deletion: images reuse the attachment path and its checks instead of a second image store, and the aborted-result literal was replaced by `finish`, which dropped 5 lines
shares: #2295 it edits crates/acp/src/update.rs and crates/runtime/src/lib.rs; the stacked #2294 below this branch adds one line there, and this branch adds nothing to it
shares: #2256 it edits crates/runtime/src/lib.rs; the stacked #2294 below this branch adds one line there, and this branch adds nothing to it
shares: #2239 it edits crates/acp/src/update.rs and acp_unit.rs; the stacked #2294 below this branch adds one line there, and this branch adds nothing to it
shares: #2237 it edits crates/runtime/src/lib.rs; the stacked #2294 below this branch adds one line there, and this branch adds nothing to it
shares: #2298 it edits crates/acp/tests/acp_unit.rs and crates/runtime/src/lib.rs; the stacked #2294 below this branch adds one line there, and this branch adds nothing to it
shares: #2296 it edits crates/acp/src/update.rs; the stacked #2294 below this branch adds one line there, and this branch adds nothing to it
raise: crate kernel +155, crate types +13, crate tools +2, tests +154, comments +5
---
A cell's displayed picture, HTML and markdown now reach the model and clients. `image/png` and `image/jpeg` from a `display()` or a cell's last value attach under the attachment checks, up to 8 a cell. `text/html` and `image/svg+xml` stay whole on the result's `displays` (262,144 characters a cell) for a client to draw. A display's `text/plain` or markdown fallback is added to the cell's output, and a refused image, a spent cap or a type nothing carries writes one `[…]` row in the model's view naming what and how many.
