---
issue: Closes #1372
raise: tests +7
---
The console reveal drive test no longer depends on how loaded the machine is (Closes #1372). When a console takes a reply and its end in one drain, the reveal switches to drain mode. The daemon's six-character faux reply then fits inside one 50 ms reveal step, so on a loaded machine no frame showed it half revealed, and three of four `just check` runs failed. The test's HOME now configures `tui.pace` 10, where every character outlasts a step even in drain mode. Its script replays the batch with two paused `type` steps, so reverting the pace fails it six runs out of six.
