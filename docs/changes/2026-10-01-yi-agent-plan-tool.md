---
issue: Closes #951
raise: crate context +7, tests +49
---
A host nudge reaches the model as runtime context, and a wide search no longer tells a question to plan (Closes #951). A grep over more than five files loaded the orchestrate protocol and sent "This task has outgrown one-shot handling; write the plan now." as a bare user-role message, so a model asked whether the classifier was implemented took the nudge for the user and planned a read-only question. An extension's reminder is now a `reminder` note, rendered as `<yi_internal_context source="reminder">` behind the advisory line, and the TUI draws it as `⚑ reminder`; the todo coupling's send-backs (`todo_intercept`) ride as `source="todo"`, where one was answered as "User asks"; the wide-search signal loads the protocol without the nudge, as the turn-end signal already did, and the protocol's first line says a question or an assessment is not a change.
