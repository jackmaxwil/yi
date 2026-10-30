---
issue: Closes #1723
raise: tests +39
---
The six link-swap race tests in `crates/runtime/tests/permission_scope.rs` waited a fixed eight seconds each and gave up on a call count nobody checked; they now run a fixed number of calls whose swap is timed to land at a different point of each one, and stop when enough swaps landed inside a call, so their wall follows the work instead of the clock. Each call's swap is armed on a deadline the swapper thread waits for and the test times against the call's own measured span, swept over sixty-four offsets, so every window between a tool's check and its act is reached without a sleep holding the test open. A test now fails when fewer than the counted swaps land inside a call, since a run that never raced proves nothing.
