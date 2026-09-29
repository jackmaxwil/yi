---
id: simplify
question: is it minimal
decider: reader
claim: Deleted / alternatives
severity: {"high": "the change re-implements something that already exists in this repository", "medium": "a real simplification, or a name a cold reader would misread on a public API, schema field, config key or command", "low": "taste"}
refute: {"high": 3, "medium": 1, "low": 1}
---
Look for what can go and what reads wrong: code that duplicates a helper already in this
repository, dead code, a special case where one guard in the shared function would do, an
abstraction with one user; a name that says something the code does not do, or a new name for a
concept the repository already names. Test the claims in "Deleted / alternatives".
