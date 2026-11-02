---
issue: Closes #1982
---
The public mirror had not moved since 2026-10-29, because its history scan refused two things the rewrite left in place. The widened home-path rule also matched a spelling with no leading hyphen in two old revisions of a plan, and `replace.txt` rewrote only the hyphen form. Two commit messages quoted the mirror test's planted address, and the tree gate reads no messages. `replace.txt` now rewrites both, with the address rules matched on the quoted text alone, so `sync_test.sh`'s planted address still trips the deny rule. No deny rule changed.
