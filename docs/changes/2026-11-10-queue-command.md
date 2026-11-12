---
raise: comments +13, tests +236, crate runtime +101, crate tui +300
ledger+: TUI | `/queue <message>` runs the message after the current turn
---

`/queue <message>` sends a follow-up. While a turn is running it waits and runs when that turn ends, instead of steering into the turn the way a typed line does. On an idle session it starts the turn. With no message it says what it needs and queues nothing. The verb is on the solo TUI popup and on ACP `_yi/slash`.
