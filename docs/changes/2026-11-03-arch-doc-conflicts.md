---
issue: Closes #2008
decision: Every direct feature-ledger edit is refused, whenever it was committed (extends D524) | 31 of the 33 unmerged jack branches editing ARCHITECTURE.md changed a feature-ledger row, and the pre-2026-11-02 exemption still let those branches conflict on recorder-owned rows | restore the commit-date exemption in check_changes.py
---
The branch guard now refuses direct feature-ledger edits from older pull requests too. All ledger additions and edits travel through the existing change-file operations, which the recorder applies on main; measurement found no hunk class that needs a new recorder operation.
