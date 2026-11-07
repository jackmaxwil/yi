---
name: classifier-routes-turns
description: The turn classifier lives in crates/types/src/classifier.rs and its label routes each turn.
type: project
---
A label is a closed vocabulary; the classifier's join key is the collapsed, lowered text.

**How to apply:** a new route adds a variant there and a fixture under evals/fixtures, never a new subsystem.
