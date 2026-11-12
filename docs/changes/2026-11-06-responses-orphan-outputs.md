---
raise: crate ai +50, tests +34
ledger=: OpenAI Responses tool-call replay | 5 | core | M | L | unmatched function and custom tool outputs become assistant text notes containing the call_id and output text; paired outputs remain protocol outputs
---
OpenAI Responses request assembly now repairs unmatched function and custom tool outputs as assistant text notes containing the call ID and output text. Outputs paired with their tool calls remain unchanged.
