---
issue: Closes #989
raise: crate runtime +10, comments +2
---
A reader's JSON answer is taken from a fence after prose (Closes #989). `rlm.result(schema=...)` refused a reply that was a paragraph followed by one fenced object matching the schema, as "answered with text, not JSON", because only a reply that began with a fence was unwrapped. When the reply does not parse whole, the last fenced block that parses as JSON is the answer, and the schema checks it as before; a reply with no parseable JSON is refused as before.
