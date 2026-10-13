---
issue: Closes #1354
decision: The dist binary hard cap moves from 8 MiB to 9 MiB (9,437,184 bytes) | origin/main sat 16,336 bytes under 8 MiB and a feature costs whole 16 KiB alignment steps, so the next landing trips the cap; the owner chose 9 MiB on 2026-10-12 | restore `HARD_CAP_BYTES` to 8_388_608 in `check_binary_size.py` once the binary is under it
---
The dist binary's hard cap is now 9 MiB (9,437,184 bytes) in place of 8 MiB (Closes #1354). The measured ratchet in `binary_size_budget.json` is unchanged at 8,372,272 bytes and still moves only by `--update` in its own commit; only the ceiling it may not be edited past moves. `check_binary_size.py`, its self-check, the push lane's hard-cap fixture in `forge_pr.py`, §18.6 of the design and the dependency rule in the agent instructions all name the new number.
