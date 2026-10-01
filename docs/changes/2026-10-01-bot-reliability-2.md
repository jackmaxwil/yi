---
issue: Refs #976
---
The review job's `pull_request_target` run has its own concurrency group: a branch that never
merged main still runs its old `pull_request` copy of the job, and on #988 that copy cancelled
the target run in the same second, failing main's required `review` check. The autofixer gives
the model one turn at the commit hook's own FAIL lines when the hook refuses a resolved merge,
then judges and commits again (Refs #976): #970's merge resolved cleanly and left `session.rs`
one line past the 1,200-line cap.
