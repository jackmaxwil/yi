---
issue: Closes #1445, Refs #1442
---
The goal's three prompts (continuation, budget limit, objective updated) are filled by plain string replacement. The 173-line strict `{{ name }}` engine in `goal/template.rs` is gone, and so is its error type: the three builders return a `String`, and the three callers in `goal/mod.rs` no longer drop the goal message when a template errors. The objective is filled last, so a literal `{{ token_budget }}` typed into it reaches the model as written instead of being rewritten as a number. The engine's only guard, that a renamed placeholder could not silently drop content, now lives in one test that renders the three shipped prompt files and fails on any `{{` left over.
