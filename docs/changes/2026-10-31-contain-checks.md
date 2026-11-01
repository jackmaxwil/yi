---
issue: Closes #1931
decision: goal checks, child checks, plan verifiers and exec:// sources run in the sandbox outside yolo | an allowed `cargo test` as a check ran a child's build.rs on the host under a green gate | set Checker's profile to None
growth: +178 includes #1945's lines beneath it; `Checker` carries the sandbox to every check site in place of the hooks handle, and its test sits in the crate. Weighed: a parameter beside `hooks` at each of nine sites, more lines; a global profile, which a family of sessions could not share
raise: tests +114, comments +13, crate runtime +149, crate tools +12
---
A check now runs where the same command would run in bash: inside the sandbox, outside yolo. Before, a goal check, a child's `check=`, a plan verifier and a schedule's `exec://` source all ran on the host, so `cargo test` as a check ran a child's `build.rs` outside the sandbox. A check the gate would only contain used to be refused at spawn; it now runs contained, so `just check` can be one.

A contained check has no unix sockets, no writes outside its tree and, until the egress proxy lands, no network past loopback. What the sandbox stopped is named: a goal or child check opens its evidence with a `[contained: …]` row, and a plan verifier abstains with the same words instead of failing.
