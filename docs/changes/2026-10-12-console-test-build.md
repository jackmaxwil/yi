---
issue: Refs #1256
raise: tests +2
---
The console drive test `paced_type_step_respects_the_deadline` builds again. It landed with #1278, after `yi_console::ConsoleOptions` gained `open_apps` and `pace` in a PR merged the same hour, so main's console test binary did not compile. The initializer now passes the defaults the other tests use.
