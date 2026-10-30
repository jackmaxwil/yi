---
version: 0.803.0
issue: Refs #1315
---
`session-mining` is listed, triggered, classified and read only in a repository holding `skills/yi/session-mining/extract.py`, the versioned extractor its method runs (Refs #1315). It was the one Yi-repository skill the `within:` key landed without, so every other repository's catalog offered a sweep recipe whose first command names a path that repository does not have.
