---
issue: Refs #1966
---
The merge queue's tick now stops with the forge's own words when the forge answers an error, instead of dying on an AttributeError that never said what came back. A forge write it was refused stops the tick too, so a landing whose issue would not close keeps its candidate ref and the next tick finishes it, where before the issue stayed open with nothing left to retry.
