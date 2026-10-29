---
issue: Closes #1718
growth: +431 additive wire metadata and one shared live/replay projection; a second renderer, timeline, console implementation and persistent terminal bytes were weighed for deletion and rejected as duplicates
decision: Subagents should be persistent in their two locations in the scrollback: at start and end | One shared formatter projects the durable spawn and end records into two concise rows, so the live card no longer needs to become the permanent end record | Remove the replay projection and restore the committed full Task cell; the added optional wire fields remain additive and old readers ignore them
raise: crate console +4, crate runtime +129, crate tools +1, crate tui +279, crate types +18, tests +972
---
A child now gets one persistent start row when it is admitted and one persistent end row for each run. The rows name the child and its role and task at the start, then its actual outcome, elapsed time, tokens, optional cost, start id, and first result or error line. The committed end row replaces the full finished card; the live card and its focus view stay unchanged. The 32-card live-tracking cap still applies only to live state, not to committed rows.

The existing `custom{child}` spawn and end entries carry the values needed to rebuild those rows on cold resume and replay. Every added field is optional, so old entries decode and reserialize unchanged and older records produce reduced rows without invented details. Reaping, repossession, and parent close now journal a fresh ending exactly once, with `parent closed` preserved as the cause.

Live updates and replay use one projection and formatter. A row whose `spawnedBy` cell has not committed waits behind that antecedent, unresolved references flush in ledger order at replay EOF, and a late end appends at finish with a back-reference to its start. Console inherits the same behavior through the shared `yi-tui` reducer; no second renderer or sidebar state is added.

A new cell variant, a second timeline, persisted terminal bytes, and retroactive field fabrication were rejected as larger designs that the existing trail and `Cell::Rule` path do not need.
