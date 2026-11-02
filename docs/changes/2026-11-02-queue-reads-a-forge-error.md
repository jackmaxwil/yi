---
version: 0.881.0
issue: Refs #1966
---
The merge queue's tick now stops with the forge's own words when a listing comes back as an error, instead of dying on an AttributeError that never said what came back. A refused issue close now keeps the candidate ref, so the next tick tries again, where before the issue stayed open with nothing left to retry. After four hours the tick says so on the PR and moves on, so one issue it may not close cannot hold up the queue.
