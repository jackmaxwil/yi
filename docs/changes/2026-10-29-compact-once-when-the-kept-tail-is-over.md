---
issue: Closes #1757
raise: crate runtime +29, tests +116, comments +2
---
Compaction no longer fires on every request when the kept tail is already over `compaction.at`. A compaction records the size of the messages it left (rebuilt from the latest compaction entry on resume or rewind), and the next ceiling compaction waits until the messages have grown past that by one `reserve`. The model window's own reserve line is not held back, so a request that would leave no room for the reply still compacts. The dogfood run that found this compacted 16 times in 48 requests and cost 66% more than the run with compaction off; the summarizer fires once now and again only when the history actually grew.
