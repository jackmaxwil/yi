---
issue: Closes #1586
raise: crate runtime +43, tests +30, comments +4
---
A whole-plan `set` that carries repairable graph problems repairs them by re-sending the set, and each pass went through the tool's real apply, so every refused pass left a refusal record in the journal before the set finally landed: a set with a duplicate label, an unresolved edge and a cycle wrote three records where the plan needs one. The repair loop now validates each pass through the reducer's dry run (`PlanEngine::dry`) and journals nothing for it; the one apply the call ends on writes the one record, and a set the table cannot settle still writes its single refusal. The dry run is the reducer's own check on a clone of the loaded state, so the refusal a pass returns is the one the real apply would give and nothing is written for it.
