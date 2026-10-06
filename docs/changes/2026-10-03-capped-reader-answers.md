---
issue: Closes #992
decision: the turn cap's last word answered with a tool call and no text gets exactly one more request, with the tools kept and tool choice `none`; when that request fails before delivering anything, its stream retry offers no tools, so a reader's schema rides it as structured output; a tool call in either reply ends the run as before, and a deadline or wind-down last word is out of scope: its run settles on the refused call and pays no request, because D299's contract there is that the last word's turn ends the run inside the reserved grace (amends D309) | in a bug-localizing fan-out two of five readers (role=reader, turns=3, GLM 5.3 flash) called `read` after the turn-cap note, the call was refused and the run ended with no text, which `rlm.result` reported as "answered with text, not JSON" with an empty body; keeping the tools keeps the cached prefix and keeps a history of tool calls valid on Anthropic, which refuses tool blocks on a request with no `tools`; D299's reason stands for the last word itself: OpenRouter routes no `z-ai/glm-5.3-flash` endpoint for `tool_choice: "none"` and answers HTTP 404, which is why the retry drops the tools instead. Trade: where `none` is accepted the schema does not ride (D309 sends it only on a tool-less request), so a reader's JSON comes back as text for `rlm.result` to parse (#989); only a route that refuses `none` gets structured output. Ceilings: such a route pays one refused request first; a follow-up that fails for any other reason (a 529, a dropped stream) is also retried tool-less, which Anthropic refuses, so that run ends with no answer as before | drop `followed_up` and the follow-up in `run_loop`; the refused call ends the run again
raise: crate loop +31, crate runtime +7, crate types +7, tests +123, comments +4
---
A capped reader that calls a tool on its last turn still answers (Closes #992). The turn-cap
note keeps the tools on the request so its prefix stays cached, and a call there is refused;
until now the run then ended with no text, so `rlm.result` returned an empty answer. When the
last word's reply is only refused tool calls, the loop sends one more request that keeps the
tools and forces tool choice `none`; on a route that refuses `none` (OpenRouter's GLM 5.3
flash) the retry offers no tools and carries the reader's answer schema. It is one request,
never a loop: a tool call in that reply ends the run as before. The follow-up rides the turn
cap's word alone: the runtime marks the word its turn-cap branch returns (`last_word_capped`)
and only that word earns the forced request, so a deadline run's wind-down still settles and
ends on the refused call, inside the grace D299 reserves for it.