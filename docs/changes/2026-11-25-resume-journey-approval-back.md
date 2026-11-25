---
version: 0.1127.0
issue: Closes #2374
---
The Yi Desktop `resume` journey now expects what the server has done since 0.1082.0: after the app quits and starts again while a turn waits on an approval, the server sends the request again, so the approval card and "1 need you" come back. A third launch answers it with Enter and asserts the command ran (`one.txt` exists, "turn done") and that nothing says the approval was lost. The journey had failed on every run against a current `yi`. The app's lost-approval notice stays for a request the server does not send again.
