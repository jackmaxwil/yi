---
issue: Closes #1350
raise: crate console +3, tests +25
---
Stopping the daemon from the console with a second ctrl+c no longer flashes a red "disconnected" error on the way out. The console used to wait a fixed two seconds after asking the daemon to stop, and painted the daemon's own exit as a lost connection in that gap; it now quits as soon as the daemon hangs up, and still gives up after two seconds when a daemon never does.
