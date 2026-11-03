---
version: 0.860.0
issue: Closes #1970
raise: crate console +4, tests +35, comments +2
---
The console no longer loses a request it queued just before quitting. On quit, the reader thread closed the shared socket as soon as it noticed shutdown, which could beat the writer thread to the last queued line, so a sidebar toggle pressed right before quitting was sometimes never saved. The writer now closes the socket after its queue is flushed.
