---
issue: Closes #2027
---
The merge queue now reads its own base and retry markers, the last ones in the candidate tip's body, and refuses a base marker that is not an ancestor of the tip in favour of the first-parent or merge-base fallback. The unused split counter is gone, and the batch, conflict and landing scenarios in the queue's self-test now drive the tick's real observer instead of a copy of it.
