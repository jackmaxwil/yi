---
version: 0.812.0
issue: Refs #1681
---
Let `pr check` take `--closes` and `--refs`, judging the body as `pr open` composes it, so a stack tool no longer writes a temporary copy of the body to carry the issue line.
