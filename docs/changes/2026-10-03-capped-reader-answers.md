---
issue: Closes #992
raise: crate loop +22, tests +96, comments +2
decision: a last word answered with a tool call and no text gets exactly one more request that offers no tools, so a reader's schema rides it as structured output; a tool call in that reply ends the run as before (amends D299, which ended the run on the refused call, and D309, under which a capped reader with tools never got the schema at the provider) | in a bug-localizing fan-out two of five readers (role=reader, turns=3, GLM 5.3 flash) called `read` after the turn-cap note, the call was refused and the run ended with no text, which `rlm.result` reported as "answered with text, not JSON" with an empty body; the normal last word keeps its tools so the request prefix stays cached | drop `tool_less` and the follow-up in `run_loop`; the refused call ends the run again
---
A capped reader that calls a tool on its last turn still answers (Closes #992). The turn-cap
note keeps the tools on the request so its prefix stays cached, and a call there is refused;
until now the run then ended with no text, so `rlm.result` returned an empty answer. When the
last word's reply is only refused tool calls, the loop sends one more request offering no
tools, which carries the reader's answer schema, and the run ends on its reply. It is one
request, never a loop: a tool call in that reply ends the run as before.
