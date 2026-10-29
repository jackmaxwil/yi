---
issue: Closes #1487
raise: tests +156, crate kernel +15, crate runtime +3
---
The runtime's two remaining silent cuts are named (unit CAPS-runtime, finishing what 0.753.0 left open on #1487). A `rlm.find_models` search that matched more than `limit` already produced a `[… kept of total matching models (limit N); the rest: await rlm.find_models("", limit=T)]` row in the host reply, and the `rlm` shim read only `models`, so the row never reached a cell and the model saw a short list with nothing to say it was short. The shim prints the row now, and its docstring says the list it returns is always the `limit` asked for.

A variable the snapshot left out came back as silence. The snapshot has always counted its skips (`KernelSnapshotResult.skipped`: over the 16 MiB per-variable cap, over the aggregate cap, or unpicklable), but only the snapshot's own marker line saw them; the resume read an index that no longer held the name and said nothing, so a revived kernel's notice claimed a namespace it did not have. The snapshot writes that skip list beside the index (`kernel-state.skips.json`), and the restore appends it to the `failed` names it reports, so the notice a resumed kernel prints names each one and why. The existing line for names that cannot be restored now carries each reason in parentheses, since a model told only `conn` will reconnect the socket that cannot be pickled rather than the one over its cap.

No type changed: `KernelSnapshotSkip` already says "one name the snapshot/restore helpers could not process, with a short reason", so a name the snapshot skipped is a name restore did not produce. The restore notice's list stays whole rather than cut — no call returns a name a cap hid, and those are the names the model has to act on.
