---
issue: Refs #1966
---
The autofix pass now takes `queue`-labelled PRs before anything else, oldest first after them. A pass fixes at most five PRs from the oldest end of the open list, so a conflicted PR the queue handed over could wait behind dozens of older ones it had no reason to wait for. A queued PR the fixer hands off now leaves the queue and says so, because the queue would otherwise ask the fixer again on every build.
