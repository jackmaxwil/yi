---
issue: Closes #820
decision: a backgrounded bash job reports only to the session that started it, and its exit starts a turn when that session is idle (amends D305): `ToolAdapter` stamps each call's `ToolContext` with the session's `JobOwner`, minted once per `AgentSession`; a `Reaper::Poller` job carries it; `Jobs::take_finished(owner)` matches on it where it matched the cwd; the completion loop delivers through `run::follow`, which starts a turn on an idle session; `AgentSession::retire` (was `dispose_kernel`) marks the session retired, and a retired session's loop ends and starts no turn; the still-running text inside a session says the exit starts the next turn and to end the turn rather than sleep or poll, and outside one that the result arrives only while the turn runs | in the owner's Claude Code sessions 121 of 180 poll with `sleep` or `pr status`, and a Yi job that exited after its turn woke nothing, so a `just pr merge` left in the background was heard only when the user typed; keyed by cwd, a reader child and its parent, or two console chats in one repository, took each other's results | key `take_finished` on the cwd again and push job reports to the follow-up queue without `run::follow`
raise: crate runtime +29, crate tools +39, tests +48
---
A background command's exit now wakes the session that started it (Closes #820). A bash call
with `wait` that is still running becomes a job; before, its `<async_result>` reached the model
only if the job exited while that turn was still running, so a `just pr merge` left in the
background was heard only after the user typed again. Now an idle session starts a turn
carrying the result, and the still-running text tells the model to end the turn rather than
sleep or poll. Results are keyed to the session that started the job, not its directory, so a
reader child and its parent, or two console chats in one repository, no longer take each
other's. A retired session (a reaped child, a respawned service's old run) starts no turn when
its job exits; a job a respawned service's old run left behind is not handed to the new one.
The doctrine's "Never sleep to wait" now says to end the turn and be woken.
