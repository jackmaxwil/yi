---
issue: Closes #1934
growth: +236 is the origin gate, the pinned recipe with its ask, and the config. Weighed for deletion: leaving recipes out, which leaves `just pr` asking on every call the owner makes; a list of known forge verbs, cut because the flags that aim a call are what decide it.
raise: tests +241, comments +14, crate cli +5, crate permission +9, crate runtime +185, crate types +10
decision: forge commands leave by home config and tree recipes by pin | a contained forge CLI has no route to its login | drop the class and sandbox.hostRecipes
---
`fgj` and `gh` now ask and an approval runs outside the sandbox, since a contained one has no login. An allow rule in `permissions.allow` runs them outside unasked, unless the call aims `-R`, `--repo`, `--hostname` or `--config` somewhere that is not this project's origin, which asks whatever the rule says.

`sandbox.hostRecipes` in the home config, such as `[{"call": "just pr", "pin": ["justfile", "scripts"]}]`, runs one command outside unasked while every pinned path matches the trunk on origin with nothing untracked. Otherwise it asks, naming the pin: a recipe a branch rewrote would run on the host. An empty `pin` list and a pin the trunk does not hold both ask, since neither pins anything, and a subagent's broker meets the same recipes its parent does. A compound never matches, so nothing rides out behind it. `just pr open` now refuses an unpushed branch instead of pushing, since a host-run push would build a child's `build.rs` outside the sandbox.
