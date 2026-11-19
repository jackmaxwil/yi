---
issue: Closes #2272
raise: tests +55, crate types +105, crate context +6, crate runtime +3, crate acp +2
---
Agent events, agent messages, child activities and setting kinds gain an `Other` case. A tag or role this build does not know decodes to it and re-emits byte for byte, so an older Yi reads a newer one's events without a typed failure. A known tag whose body does not decode is still an error, not `Other`. `ChildActivity` drops `Copy` because an unknown activity is a string. The context, runtime and TUI paths show an unknown message as a notice, and an unknown setting kind shows no value.
