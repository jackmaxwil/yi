---
issue: Closes #1064
raise: tests +45
---
The two wake tests no longer read the wall clock. `a_family_wait_wakes_on_the_move_not_on_a_poll` timed five spawn-and-wait rounds against 250 ms and went red at load average 40–60 (522 ms, 510 ms) while passing alone in 30–100 ms, so `just check` failed on machine load rather than code. Both tests now park a wait, make the move themselves (an interrupt of a running child, a routed mail) and require the parked task to finish within eight yields of the current-thread runtime. A notify wake makes the task ready at once; a 100 ms poll leaves it asleep on its timer, which no amount of contention can make fire inside that window. Thirty runs at load average 43 all passed; with the poll put back in either wait, each test fails on its wake assertion.
