---
issue: Closes #1554
raise: tests +81
---
A child's `kernel://main/<var>` read queues behind the parent's running cell and answers `Busy` after 5 s, exactly when the orchestrate protocol tells children to read it. IPython runs one execute at a time, so the read cannot be served live. `VariableReadError::Busy` now names the route that works: the agent `rlm.put`s the value before it awaits, and the reader takes `family://<name>` or `rlm.get`. The orchestrate prompt says the live read needs an idle parent.
