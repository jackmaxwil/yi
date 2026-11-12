---
issue: Closes #2143
raise: crate ai +29, tests +79, comments +2
shares: #2136 the GitHub #6 import draft this splits out; its Responses-only orphan test is replaced here
shares: #2137 stacked on the #6 import, so it carries the same Responses orphan test
ledger=: tool results left without their call | 5 | core | M | L | a replay that drops a failed or stopped reply drops the calls it carried, and each of their results becomes a user-role notice naming the tool, why its output was left out and that running the tool again gets it, on every wire; the output itself never reaches the request, in any role
---
A request no longer carries a tool result whose call it does not carry. The replay drops a reply
that failed or was stopped, and with it the tool calls it held, but it kept those calls' results,
so Anthropic messages, OpenAI completions and OpenAI responses each received a result with no
call and refused the request, on that turn and every turn after it. Each such result is now a
host notice in the user role, `[bash result omitted: its call was in a reply that failed or was
stopped, which this request leaves out; run the tool again for its output]`, sent after the
results that do have their call. The tool's own output is left out: replayed as user or
assistant text, a fetched page would read as the person's or the model's own words.

The repair is in `transform_messages`, the one replay every wire's encoder runs, rather than in
the Responses encoder alone, where GitHub #6 first fixed it. Credit: Max Ludwick found the
refusal and wrote the first repair.
