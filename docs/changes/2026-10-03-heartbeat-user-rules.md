---
issue: Closes #1003
raise: crate runtime +10, tests +37, comments +1
---
A heartbeat's `exec://` source meets the user's gate rules (Closes #1003). The heartbeat gate judged a source by the wall's host check and the permission broker but skipped the rules `.yi/rules` arms for a todo's address, so a rule denying a command did not stop a heartbeat running it on the host every interval. Both now reach that check in `host_wall`, and a source a rule names is refused at creation with `Denied by rule …`; judging the rules again at each fire is #1045.
