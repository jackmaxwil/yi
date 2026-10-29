---
version: 0.781.0
issue: Refs #1509
raise: crate runtime +56
---

A schema-bearing reader no longer repairs a valid task answer when ordinary mail arrives before that answer is persisted. The schema-answer selector keeps its existing boundary when an answer already precedes the mail; otherwise it selects the first later answer instead of validating an empty slice and scheduling a false retry. The regression covers that message order directly, and the mid-run mail integration test now waits for the session to go idle before reading the transcript.
