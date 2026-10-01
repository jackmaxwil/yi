---
issue: Closes #983
raise: crate runtime +29, tests +49, comments +2
---
A lane slot whose repository was deleted and recreated at its path is a free slot again (Closes #983). Its `.git` named a gitdir that no longer existed, so the pool's `git status` and `git checkout` in it exited 128 and every later `yi ask` in the project refused at startup with `[refusal:lane]`; only `--here` got past it. A claim now sees a slot whose gitdir is gone, stops its warmer, removes the tree and its state, and adds a fresh worktree there. Its branch went with the old repository; any uncommitted files left in that tree are removed with it, except when the session that left it resumes: that claim is refused by name and the tree is kept, since it holds the only copy of the work.
