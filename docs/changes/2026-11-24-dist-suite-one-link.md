---
shares: #2344 both edit justfile: this branch gives the dist-suite recipe an args parameter, #2344 adds a desktop recipe; whichever lands second merges main
---
The nightly `dist-suite` on the forge links one test binary at a time. Two fat-LTO test links at once exceeded the runner job's 5 GB and rustc was killed every night from 2026-11-18. `just dist-suite` now passes extra arguments to nextest, and the workflow calls it with `--build-jobs 1`. A local run and the shipped dist profile are unchanged.
