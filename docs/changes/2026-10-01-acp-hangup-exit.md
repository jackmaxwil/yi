---
issue: Closes #1010
raise: crate acp +10, tests +55, comments +3
---
`yi acp` exits when its client hangs up while a permission ask is open (Closes #1010). The ask waited on stdin for a reply that could no longer come, and the runtime's shutdown waited on the ask, so the process never ended; on Linux every bash call asks, and the strict-client test that closes a session mid-tool timed out on the gate in a share of runs. When stdin ends, every open ask is now answered as a refusal, and an ask made after that refuses at once, so the tool reports the refusal and the process exits.
