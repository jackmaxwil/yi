---
issue: Closes #1163
raise: crate runtime +2
---
The age at which an old tool result may be reclaimed is now an eval lever, `reclaim.keep_turns` (default 5; 1000 turns reclaim off), so a run can measure reclaim against its absence (Closes #1163). The first A/B on glm-5.3 (eval-ledger 0080, 0081) passed 9 of 16 in both arms and cost 13% less with reclaim on, through fewer turns; the default stays 5. The same round measured per-role models a second time (0082) at more than twice the cost per pass, so model bindings were not built.
