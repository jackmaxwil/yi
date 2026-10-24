---
issue: Closes #1459, Closes #137, Refs #1442
raise: tests +179, comments +4, crate runtime +27
decision: A channel subscription with no adapter is peeked before it is claimed, and an idle one re-arms in memory with no claim record and no write (amends the section 15.2 'every mutation persists' contract) | an idle mailbox cost two fsynced ledger rewrites a minute per durable root; nothing is delivered on an idle tick, so claim-before-deliver still holds where it matters | route every tick through claim_due_in_state again
---
A due channel job whose channel no adapter feeds (every mailbox, any `channel://` source) is peeked before the timer claims it: when the channel holds nothing past the job's ack, the job re-arms its next run in memory, with no claim and no write of `scheduled-jobs.json`. An idle mailbox used to claim, find nothing, and record the result, two fsynced rewrites of the ledger per cadence for every durable root. A letter, and every tick of a subscription with an adapter behind it, which must keep its adapter running, still claims before it delivers. The persisted `next_run_at` of an idle job is now a hint the next real write or a restart re-derives, so a crash costs one idle tick fired again. YI_DESIGN.md section 15.2 says so in its channel-job line.

Member units: H42-idle-poll.
