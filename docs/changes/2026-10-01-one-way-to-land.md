---
issue: Closes #839
raise: crate tools +33, tests +77
---
`write` and a grep apply land a file through one `land`, the way `edit` already did (Closes #839). A grep apply that breaks a file now says `a.py: syntax: error line 1: …` and carries `details.patch` for the diff pane, where it carried neither; `write` refuses a path that is a symlink, as `edit` does, instead of writing its target. `edit`'s dead create arm (`SectionOp::Create`, never produced) and the always-zero `created` count in its `ops` details are gone.
