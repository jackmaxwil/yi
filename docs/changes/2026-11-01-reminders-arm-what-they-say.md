---
issue: Refs #1992
growth: +214 weighed the day words and clock shapes against the twin `parse_iso_ms` this branch deleted, and the two-way parse table the note guard replaced
raise: crate runtime +214, tests +429, comments +16
decision: A reminder time naming no offset is wall time in the local zone, and an offset is honoured (amends D283's `at <ISO>`, read as UTC with offsets refused) | GLM 5.3 flash wrote `-08:00` on every timed reminder, and the refusal sent it to a naive time that fired at once or to a blocker nothing fires | read a naive `at` as UTC again in `parse_local_ms`
ledger+: heartbeats: clock subscriptions | `clock://at` honours an offset and reads a time with none as local wall time, a `file://` wait with no filter holds until the file exists, and `block` refuses a time with no scheme, a day word, or an address or time left outside `on`, so a reminder that armed nothing cannot read as set
---
A reminder the model sets now fires when it says. `clock://at` takes an offset, and a time with none is local process-zone wall time, which the arming seam reads. A `file://` wait holds until the file exists. `block` refuses a time missing its scheme, a week- or day-relative word, and an address or a time left in `note` or `block`; any other op refuses `on`. A reminder that armed nothing can no longer read as set, and each refusal names an address the next call can use. The `todo` description says a reminder is a blocked todo and names the route for a repeating one, so the model stops reaching for the machine's crontab.
