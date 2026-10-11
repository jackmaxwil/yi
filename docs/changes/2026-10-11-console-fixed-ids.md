---
issue: Closes #1257
raise: comments +3, crate session +44
decision: `YI_FIXED_IDS`, read by `IdGenerator::session_ids`, makes the session ids a repo mints a process-wide counter (`01a00000-0000-7000-8000-<n>`, from 1) instead of random; only the TUI scene runner sets it | the id hashes to a console session's avatar and badge colours, so every console snap differed in a few cells between any two runs and the PR comment had to say to read past them | delete `FIXED_IDS_VAR`, `IdGenerator::session_ids` and the `fixed` field; `JsonlRepo::new` calls `IdGenerator::new` again
---
Two runs of the same console scene now draw the same colours (Closes #1257). A console session's id picks its avatar and badge colours, and `yi serve` minted random ids, so every console snap changed a few colour cells between runs. `tui_see.py` now starts the server with `YI_FIXED_IDS=1`, which makes the session ids a counter from 1, and the PR comment no longer asks the reader to read past those cells. A normal run never sets the variable and mints the same random ids as before; no serialized shape changes.
