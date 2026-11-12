---
issue: Closes #2142
growth: +277 the /queue verb and its three tests are 110 runtime lines; the command list that edits the box, Tab and Enter are 167 TUI lines. Weighed for deletion: Max's duplicate popup Tab arm, its complete_tab and two popup tests, and a private-layout test were cut, and the verb reuses follow_up_message rather than a queue of its own.
raise: comments +12, tests +222, crate runtime +110, crate tui +167
ledger+: TUI | `/queue <message>` runs the message after the current turn, and starts the turn on an idle session even from the TUI's own slash thread
---

`/queue <message>` sends a follow-up. While a turn is running it waits and runs when that turn ends, instead of steering into the turn the way a typed line does. On an idle session it starts the turn. With no message it says what it needs and queues nothing. The verb is on the solo TUI list and on ACP `_yi/slash`. The solo TUI runs a slash verb on a plain thread, which had no runtime to spawn the turn on; the thread now enters the driver's runtime first.
