---
issue: Closes #1549
raise: crate types +59, tests +96
---
A tool call's arguments stay readable while the model streams them (Closes #1549). A partial call that ended inside its next key was closed as a key with no value, which is not JSON, so the salvage fell through to an empty object and the TUI preview lost every argument it had already shown until the next value started. `close_partial` now reads the member it is inside: a key with its colon takes null, a key without one is dropped with the comma that introduced it, and a string cut inside an escape keeps the characters before the backslash instead of inventing one. Every prefix of two recorded calls now salvages an object whose keys all belong to the finished call and whose completed count never falls as the stream grows, held by `json_salvage_prefix::every_prefix_of_a_real_tool_call_salvages_a_consistent_object`.
