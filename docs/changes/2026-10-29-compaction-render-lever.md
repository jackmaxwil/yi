---
version: 0.847.0
issue: Closes #1574
raise: crate runtime +23, comments +6, tests +77
---
An eval run can compact without a summarizer: `compaction.render` (0..=1, tunable, default 0) makes `Compactor::maybe_compact` compose the checkpoint from the `<yi_compact_view>` and the file lists that a summary would have been prefixed with anyway, and it opens no summarizer stream at all (Closes #1574). Nothing else about a compaction changes: the same cut, the same retained tail, the same `details`, and the elision path is untouched. The default stays 0, because the kill test that would flip it keeps planted identifiers per compaction over at least ten compactions a mode and needs the paid provider; no run here measured it, so the lever ships off and the measurement is still owed.
