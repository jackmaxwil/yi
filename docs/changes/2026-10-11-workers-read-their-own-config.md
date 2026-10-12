---
issue: Closes #1233
raise: crate cli +7, comments +2, tests +62
---
A daemon's workers read the model and the permission mode from the config instead of the daemon's start-time copy (Closes #1233). `yi serve` forwarded `--model` and one of `--auto`, `--confirm` or `--yolo` to every `yi acp --cwd <root>` worker it spawned, with whatever values it had read from `~/.yi/config.json` when it started, so an edit to `model` or `permissions.mode` never reached a new root until the daemon was restarted. It now forwards each only when it was named on the daemon's own command line, and a worker otherwise reads the config itself. A config the strict load refuses now stops the next root's worker rather than the daemon, and the client reads `worker exited`. A running daemon still ignores `yi console --yolo`, since the console's flags reach the daemon only when the console starts it.
