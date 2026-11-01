---
issue: Closes #1723
raise: tests +58
---
The six link-swap race tests in `crates/runtime/tests/permission_scope.rs` waited a fixed eight seconds each and gave up on a call count nobody checked; they now run a bounded number of calls whose swap is timed to land at a different point of each one, and stop when enough swaps landed between the broker's check and the tool's open, so their wall follows the work instead of the clock. Each call's swap is armed on a deadline the swapper thread waits for and the test times against the median span of the check plus the call, swept over sixty-four offsets up to twice that span, so every window between a tool's check and its act is reached without a sleep holding the test open. A swap counts only when it landed after the check and changed what the tool returned, since one landing after the open leaves the output undisturbed. A test now fails when too few swaps land in that window, since a run that never raced proves nothing.
