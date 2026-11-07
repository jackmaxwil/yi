---
issue: Closes #2058
decision: a lane session's status row names the repo and the branch, each beside a dim label, and the console shows the session id there too, so the row is the repo, branch and id together (amends D435, which named the repo alone) | the owner, looking at the console chat, said the worktree name and the session id were not marked and users could not tell what they were looking at; the row showed a bare `yi` and a bare `01a1b3a9`, and a lane branch clipped from the left to the tail of its uuid | restore the prior status-row layout
ledger+: TUI | the status row labels what it names: `dir` or `worktree` before the place, `branch` before the branch and `session` before the id tile, and the console's chat shows the session's id there where it hid it; a lane branch yields before the session suffix and meters yield before labels on narrow rows (`tui_unit::status_labels_the_worktree_branch_and_session_and_keeps_a_lane_branch_head`, T0; `chat::tests::a_lane_chat_status_row_labels_the_session_id_and_the_branch`, T0)
raise: crate console +32, crate tui +5, tests +115
---
The status row now says what each part is. Before, a console chat showed a bare `yi` and no session id at all, and a lane's branch was cut from the left down to the end of its long id, so nobody could tell which session, worktree or branch they were looking at.

Now the row reads `worktree yi › branch ⎇ yi/01a1…c1059` on the left and `session 01 1b9c1059` on the right. A plain checkout shows `dir <path>` in the same place. The console's chat shows the id too, which it used to hide because the pane title carries the session's name. A narrow window drops meters before labels, and drops the branch before the session suffix.
