---
issue: #2181
raise: crate tools +60, tests +59
---
A container sweep that killed what it found now reports itself as completed instead of "failed with exit code 1", and a sweep that exits cleanly in the instant its 5 s bound fires is no longer reported as a hang. The plan guide's refusal sentence now says done runs its check below plan.done_refusal_cap and is refused only once a todo already carries a red verdict one short of the cap.
