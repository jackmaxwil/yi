---
version: 0.768.0
issue: Refs #1498
growth: +2 the producer gate that limits cut-row preservation to shell cells; weighed for deletion: caller-side reassembly in the ipython tool, which spread one output contract across two modules
raise: crate tools +2, tests +14
---
Preserve a kernel cut row through the shared output view only for shell cells without a spill pointer. Background job output that merely starts with the same prefix stays in the ordinary middle cut, and regression tests cover both paths.
