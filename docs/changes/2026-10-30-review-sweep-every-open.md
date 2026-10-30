---
issue: Closes #1897
---
The review sweep reaches every open pull request, not the first 50 the forge serves: `pr_review.open_pulls` pages the listing `SWEEP_PAGE` at a time, `SWEEP_HEADS` rises from 3 to 8, and the notice names heads read over draft heads waiting. A head whose last round lost a lens or refuter answer is a sweep target, as a head no round has read already was, so the lost answer is retried without a person running `just pr review --again`. A lens or refuter call that fails on a rate limit or an overload waits `THROTTLE_DELAY` between its `THROTTLE_RETRIES` attempts, so a provider 429 is waited out inside the round instead of recorded as a lost answer.
