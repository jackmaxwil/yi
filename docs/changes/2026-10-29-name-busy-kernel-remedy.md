---
issue: Closes #1554
raise: tests +107
---
A child's `kernel://main/<var>` read queues behind the parent's running cell and answers `Busy` after 5 s, exactly when the orchestrate protocol tells children to read it. IPython runs one execute at a time, so the read cannot be served live. `VariableReadError::Busy` now names the route that works: the agent `rlm.put`s the value before it awaits, a member with a kernel reads it back with `rlm.get`, and a reader with no kernel takes a computed value through `context_keys` (`family://<name>` is the put's sidecar, not the object). The orchestrate prompt says the live read needs an idle parent.
