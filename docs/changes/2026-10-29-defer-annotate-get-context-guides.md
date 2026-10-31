---
issue: Closes #1565
decision: `annotate` and `get_context` join the deferred-guide set | their descriptions are read on every turn and the tool table is byte-constant for a session, so the bytes move to `read("yi://tools/<name>")` as `edit`, `answer`, `plan`, `todo` and `ask_user` already do | restore each description from its guide constant and drop the two `TOOL_GUIDES` rows
raise: crate runtime +3, crate tools +2, tests +46
---
`annotate`'s and `get_context`'s descriptions were the two longest left in the tool table, and both were read by every turn whether or not the tool was called. Each is now a one-line summary naming `read("yi://tools/<name>")`, its full schema unchanged, and a `TOOL_GUIDES` row serves the text it used to carry whole.
