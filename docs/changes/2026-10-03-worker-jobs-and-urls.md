---
issue: Closes #936
raise: crate runtime +10, tests +57
---
A worker that names `bash` now hears its backgrounded jobs finish, and a worker's or reader's `read` resolves `history://` and `plan://` URLs (Closes #936). A reader built by `reader::session` was given neither wiring the root child gets from `attach_runtime`; it now reuses `wire_job_completions` when `bash` is among its tools and `route_urls` over a resolver walled as the child is.
