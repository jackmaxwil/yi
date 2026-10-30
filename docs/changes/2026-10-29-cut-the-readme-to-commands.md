---
issue: Closes #1861
raise: tests +36
---
The README is commands first: every section opens with the block a reader copies, and the prose under it is one to three lines (Closes #1861). It is 7208 characters where it was 10802, 66.7 percent, with the same twelve sections and the same capabilities, no paragraph longer than two lines and none of the opening prose restating its heading. A new contract test, `readme_fenced_commands_name_verbs_help_prints`, reads every fenced `yi <verb>` in the README and fails when `yi --help` prints no such verb, so a renamed verb cannot leave a command a reader copies into an error.
