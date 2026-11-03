---
version: 0.890.0
issue: Refs #1966
---
The queue tick now carries on when the recorder refuses. docs/ARCHITECTURE.md reached the 512000-byte blob cap, `record_changes.py` refused to write past it, and `record_main` ran it with `check=True`, so every tick that had a candidate to build crashed with `CalledProcessError` and nothing could land.
