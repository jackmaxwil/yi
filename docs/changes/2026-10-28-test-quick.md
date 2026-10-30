---
version: 0.818.0
decisions: D511
issue: Closes #1724
decision: `just test-quick` is an opt-in local loop that excludes tests measured at 5 seconds or more, while `just check`, prepush and CI keep the complete suite; `just slow-tests` regenerates the exact binary-and-test filter from a fresh nextest JUnit report | the owner approved skipping the measured slow tests for the inner loop: "Yes (Recommended)" | remove the quick profiles and recipes; the full suite is unchanged
raise: tests +1
---
The development loop gains a measured quick suite (Closes #1724). The initial filter combines the cited run's 67 tests at 5 seconds or more with this checkout's timing run, 77 tests in the union. A standard-library script regenerates exact binary-and-test matchers from JUnit and proves that the full `just test` recipe does not select the quick filter. The gate runs only its refutes, which build nothing and need no cargo-nextest; the freshness check that lists the workspace runs in `just test-quick`, where nextest is required anyway. A listed row this platform does not run is excused only by a `#[cfg(` on that function in the row's own crate, so a renamed or moved test still reads as stale. Both linked-table contracts now run only in their narrow-feature lane, including the serial Cargo fallback, and the suite's HOME guard and interpreter ladder each moved into one script. The yi-permission dev override was measured and rejected: its three target tests improved only 1.064x to 1.091x, and the one-line rebuild added 25.55 seconds, missing both acceptance bars.
