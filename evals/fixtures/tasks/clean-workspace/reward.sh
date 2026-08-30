#!/bin/sh
# The graded tree is the task's own repo plus answer.txt. A SWE-Atlas patch
# reward is a `git diff` or a clean-tree assertion, so a runner artifact left
# here is scored as part of the solution on every rollout.
test "$(ls -A)" = "answer.txt
marker.txt"
