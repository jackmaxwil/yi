---
issue: Closes #1988
raise: crate permission +16, crate runtime +1, crate tools +10, tests +59, comments +2
---
Edit and grep are proven against a swapped link deterministically rather than by racing a thread: a test seam on the gate swaps `d` to a link to the key store just before each open the tool makes, and the test pins how many opens that is, so a tool that opened past the gate fails on the count. The two race tests they replace missed a broken guard about half the time, and they took about 10 s between them where these take 18 ms each. No behaviour changes.
