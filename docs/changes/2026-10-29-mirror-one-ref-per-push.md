---
version: 0.839.0
issue: Closes #1792
---
The mirror pushes one ref at a time. GitHub refused the first run with the new token (GH013: pushes can not update more than 2 branches or tags) because `scripts/mirror/sync.sh` sent main and both moved tags in one push; it now pushes main, then each tag, and `sync_test.sh`'s scratch GitHub repo refuses a push that updates more than two refs.
