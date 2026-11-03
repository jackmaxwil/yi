---
version: 0.800.0
issue: Closes #1772
---
The public mirror's deny list matched the home path only with forward slashes. A trial build from a scratchpad directory embedded `-Users-home-`, the hyphen encoding some tool directories use, and neither `scripts/mirror/scan.py` nor the mirror's history scan refused it. `deny.txt` now matches the home path with any separator, case aside, so the slashed, hyphenated and backslashed spellings are one rule; `replace.txt` rewrites the hyphen form to `-Users-home` for the text the mirror publishes. filter-repo substitutes nothing inside a binary blob, so there the release scan refuses it: `scan.py --selfcheck` now carries the string in both a text file and a packed binary, and each fixture was red before the rule changed.
