---
issue: Closes #1757
raise: crate runtime +19, tests +50, comments +2
---
Compaction no longer fires on every request when the kept tail is already over `compaction.at`. A compaction records the request size it left, prefix included, and the next one waits until a request has grown past that by one `reserve` — or until the request reaches the model's own context window, which changes nothing for a history already at the ceiling. The dogfood run that found this compacted 16 times in 48 requests and cost 66% more than the run with compaction off; the summarizer fires once now and again only when the history actually grew.
