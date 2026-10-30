---
issue: Closes #1538
decision: A contained member's kernel writes no family board, and its `kernel://` objects read as text | D240 granted the board to every child so D164's upward channel reached the root, but the grant is also write access to the one directory the root's unconfined kernel unpickles from, and the child it protects is exactly the one with a wall, a container or a checkout of its own | restore the grant in `session_sandbox` and let `Resolver::dump_kernel` hand back its path again
raise: crate runtime +58, tests +221, comments +12
---

# Stop a parent kernel unpickling what a contained child wrote

D240 gave every child's kernel the family board as a writable root, so `rlm.put` in a child
reached `rlm.get` in its parent. The same grant is write access to the directory the parent's
kernel calls `dill.load` on: a walled child — the member D240 was protecting — could dill a
`__reduce__` to the board, and the root's unconfined kernel ran it on the next
`rlm.fetch("kernel://<child>/<var>")`.

`Wall::contained` now answers the question both ends ask: any wall, a container, or a checkout
of its own (`isolated`, set from the spawn's isolation and inherited through `Wall::under`).
`session_sandbox` names the board in that member's `deny_write`, which outranks the writable
root `kernel_profile` grants: the child's kernel meets `EPERM` on `rlm.put`, and so does its
`bash()`.

Reading up still works, as text. `Resolver::dump_kernel` asks the member's own kernel whether
it may write the board (`KernelService::writes_board`, the same deny read back), and answers
`None` when it may not; the idling host handler falls through to the ordinary text read,
which `rlm.fetch` already returns when a reply carries no path. So a parent can still read what
a contained child computed, and never loads its pickle.

The `fetch` host handler moved from `wiring.rs` to `fetch::serve`: `wiring.rs` is past the
1,000-line ceiling and may not grow, and the handler was the fetch layer's own.
