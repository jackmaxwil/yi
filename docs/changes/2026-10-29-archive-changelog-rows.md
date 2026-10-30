---
issue: Closes #1743
---
`scripts/record_changes.py` keeps `docs/CHANGELOG.md` under the 512000-byte blob cap itself (Closes #1743). When recording would leave the changelog past the cap, or it already is past it, the recorder moves the oldest rows verbatim into a new `docs/archive/changelog-<lo>-to-<hi>.md`, keeps the newest rows that fit in three quarters of the cap, and adds a pointer line to the new archive beside the existing ones. A log that archiving cannot bring under the cap is left alone and the existing refusal names it. This run archives rows 0.250.0 to 0.534.0, taking the changelog from 852141 to 384106 bytes, so the `docs/CHANGELOG.md` entry leaves the blob allowlist.
