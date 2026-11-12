---
version: 0.1030.0
issue: Closes #2143
raise: crate ai +35, tests +62, comments +2
shares: #2136 the GitHub #6 import draft this splits out; its Responses-only repair is replaced here
shares: #2137 stacked on the #6 import, so it carries the same Responses-only repair
ledger=: tool results left without their call | 5 | core | M | L | a tool result that does not answer, once, a call of the turn just before it (its call was in a failed or stopped reply the replay drops, or a note closed its call's turn and the call was already answered as `No result provided`) becomes a user-role notice naming the tool, that no call before it still awaits it and that running the tool again gets its output, on every wire; the output itself never reaches the request, in any role
---
A request no longer carries a tool result that does not answer a call of the turn just before
it. The replay drops a reply that failed or was stopped, and with it the tool calls it held, but
it kept those calls' results, so Anthropic messages, OpenAI completions and OpenAI responses
each received a result with no call and refused the request, on that turn and every turn after
it. A result that arrived after a note had closed its call's turn was a second answer, beside
the `No result provided` the replay already gave that call, and was refused the same way.

Each such result is now a host notice in the user role, `[bash result omitted: no call before it
in this request still awaits it; run the tool again for its output]`, sent after the results
that do answer their call. The tool's own output is left out: replayed as user or assistant
text, a fetched page would read as the person's or the model's own words. The tool name in the
notice keeps only letters, digits and `_-.:`, at most 64 of them.

The repair is in `transform_messages`, the one replay every wire's encoder runs, rather than in
the Responses encoder alone, where GitHub #6 first fixed it. Credit: Max Ludwick found the
refusal and wrote the first repair.
