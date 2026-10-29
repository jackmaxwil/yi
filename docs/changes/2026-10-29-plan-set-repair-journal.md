---
issue: Closes #1586
raise: comments +8
raise: crate runtime +76
raise: tests +30
---
A whole-plan `set` that carries repairable graph problems repairs them by re-sending the set, and each pass went through the tool's real apply, so every refused pass left a refusal record in the journal before the set finally landed: a set with a duplicate label, an unresolved edge and a cycle wrote three records where the plan needs one. The repair loop now validates each pass through the reducer's dry run (`PlanEngine::dry`) and journals nothing for it; the one apply the call ends on writes the one record, and a set the table cannot settle still writes its single refusal. `checkpoint_family` compares a rendered checkpoint with the file it would replace and skips the write when the bytes are already there, so a refresh that changes nothing fsyncs nothing.
