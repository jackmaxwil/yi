---
issue: Closes #1797
---
The public mirror never carries `scripts/intake/allowed.json`, the owner's contributor list. `scripts/mirror/exclude.txt` drops it at every commit, `deny.txt` refuses any fox.com address in a tree that reaches the scan, and `sync_test.sh` fails when the file reaches the scratch mirror.
