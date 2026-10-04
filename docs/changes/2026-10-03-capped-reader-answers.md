---
issue: Closes #992
raise: crate loop +23, tests +125, comments +2
decision: a last word answered with a tool call and no text gets exactly one more request, with the tools kept and tool choice `none`; when that request fails before delivering anything, its stream retry offers no tools, so a reader's schema rides it as structured output; a tool call in either reply ends the run as before (amends D299, which ended the run on the refused call and never forced a choice, and D309) | in a bug-localizing fan-out two of five readers (role=reader, turns=3, GLM 5.3 flash) called `read` after the turn-cap note, the call was refused and the run ended with no text, which `rlm.result` reported as "answered with text, not JSON" with an empty body; keeping the tools keeps the cached prefix and keeps a history of tool calls valid on Anthropic, which refuses tool blocks on a request with no `tools`; D299's reason stands for the last word itself: OpenRouter routes no `z-ai/glm-5.3-flash` endpoint for `tool_choice: "none"` and answers HTTP 404, which is why the retry drops the tools instead. Trade: where `none` is accepted the schema does not ride (D309 sends it only on a tool-less request), so a reader's JSON comes back as text for `rlm.result` to parse (#989); only a route that refuses `none` gets structured output. Ceilings: such a route pays one refused request first; a follow-up that fails for any other reason (a 529, a dropped stream) is also retried tool-less, which Anthropic refuses, so that run ends with no answer as before | drop `followed_up` and the follow-up in `run_loop`; the refused call ends the run again
---
A capped reader that calls a tool on its last turn still answers (Closes #992). The turn-cap
note keeps the tools on the request so its prefix stays cached, and a call there is refused;
until now the run then ended with no text, so `rlm.result` returned an empty answer. When the
last word's reply is only refused tool calls, the loop sends one more request that keeps the
tools and forces tool choice `none`; on a route that refuses `none` (OpenRouter's GLM 5.3
flash) the retry offers no tools and carries the reader's answer schema. It is one request,
never a loop: a tool call in that reply ends the run as before.
