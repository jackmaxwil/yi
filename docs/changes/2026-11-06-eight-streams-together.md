---
version: 0.967.0
issue: Closes #2056
raise: tests +12
---
The connection reuse test now holds every reply until eight requests are waiting, so its "eight concurrent streams" really are open together. On a loaded queue runner one stream used to finish and lend its connection before the others dialed, so the test counted six or seven accepts and ejected merge-queue candidates that had nothing to do with it.
