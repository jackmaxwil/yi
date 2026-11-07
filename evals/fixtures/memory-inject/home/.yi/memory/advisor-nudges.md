---
name: advisor-nudges
description: The advisor lives in crates/runtime/src/advisor and nudges a long turn toward a plan.
type: reference
---
It reads the turn's shape, not its content, and speaks once per settle.

**How to apply:** a feature that wants a nudge registers there; the reply carries the advisor's own words in the transcript.
