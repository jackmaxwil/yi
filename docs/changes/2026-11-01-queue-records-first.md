---
issue: Refs #1966
---
The merge queue's tick now records whatever main has landed before it builds a candidate. A recorder job waiting on a runner pushed while one candidate went green, which moved main and threw that gate run away. The later job now finds nothing to push.
