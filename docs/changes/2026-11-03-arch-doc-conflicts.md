---
issue: Closes #2008
decision: Every direct feature-ledger edit is refused, whenever it was committed (extends D524) | 31 of the 33 unmerged jack branches editing ARCHITECTURE.md changed a feature-ledger row, and the pre-2026-11-02 exemption still let those branches conflict on recorder-owned rows | revert the whole change: restore the commit-date exemption in scripts/guardrails/check_changes.py together with the old-edit refusal in scripts/record_changes.py's ledger selfcheck, which expects that edit to be refused
raise: crate runtime +18, tests +2
---
The branch guard now refuses direct feature-ledger edits from older pull requests too. All ledger additions and edits travel through the existing change-file operations, which the recorder applies on main; measurement found no hunk class that needs a new recorder operation.

Seen red: `scripts/record_changes.py::ledger_selfcheck` commits a direct feature-ledger edit on the `old-edit` scratch branch at 2026-10-20, then calls `refuses("an older direct edit", ...)`. Against the pre-change guard, `python3 scripts/record_changes.py --selfcheck` failed with `an older direct edit read 0`; the fixed guard refuses the edit with a `ledger~:` or `ledger+:` repair line. The fixture is the recorder's real twelve-file landing history at `PIN`.
