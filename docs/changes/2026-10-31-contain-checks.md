---
issue: Closes #1931
growth: +314 includes #1945's lines beneath it; `Checker` carries the sandbox to every check site in place of the hooks handle, and its tests sit in its crate. One placement function serves bash and a check (`jobs::place`) and one sweep rule serves both (`jobs::swept`), so the precedence and the sweep have one owner each. Weighed: a parameter beside `hooks` at each of nine sites, more lines; a global profile, which a family of sessions could not share; a second copy of the placement in the goal module, which is where the drift came from
raise: tests +270, comments +23, crate runtime +214, crate tools +91
---
A check now runs where the same command would run in bash: inside the sandbox, outside yolo. Before, a goal check, a child's `check=`, a plan verifier and a schedule's `exec://` source all ran on the host, so `cargo test` as a check ran a child's `build.rs` outside the sandbox. A check the gate would only contain used to be refused at spawn; it now runs contained, so `just check` can be one.

A contained check has no unix sockets, no writes outside its tree and, until the egress proxy lands, no network past loopback. What the sandbox stopped is named: a goal or child check opens its evidence with a `[contained: …]` row, and a plan verifier abstains with the same words instead of failing.

The placement itself is `jobs::place`'s: the container, else the sandbox, else plain, the same order bash takes, so a `container:<image>` child's check runs in its container and never under the host's profile. A check placed in a container keeps the variables its host client finds that daemon by, which `env_clear()` had been taking away, and is probed for syntax only: a program the image carries and the host does not is not a refusal.

What a killed check left running is swept under a bound, and the sweep runs before the capture settles, so a lane settle never reads a check gone quiet while its container still writes the tree. A directory is granted and admitted by one predicate, so `--allow-write .` is not taken and then dropped a step later.
