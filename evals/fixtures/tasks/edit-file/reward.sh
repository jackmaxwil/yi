#!/bin/sh
# The workspace copy is scored, never answer.txt: the echo carries the needle
# too, so grading the answer here would score the prompt back to itself.
test -f src/done.txt && grep -qx 'PORTAGE-9' src/done.txt
