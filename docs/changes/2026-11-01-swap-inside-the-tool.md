---
issue: Closes #1988
raise: crate permission +16, crate runtime +1, crate tools +10, tests +66, comments +2
---
Edit and grep are proven against a swapped link deterministically rather than by racing a thread: a test seam on the gate swaps `d` to a link to the key store just before each open the tool makes, and the test pins how many opens that is, so a tool that skips the gate for an open fails on the count; a tool that reopens by path after the gate does not. The two race tests they replace could miss a broken guard in a run, and they took about 10 s between them where these take tens of milliseconds each. No behaviour changes.
