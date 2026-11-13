---
issue: Closes #2161
raise: tests +35
---
Web search no longer throws away an answer because one of its citations has no excerpt. OpenRouter
documents `url_citation.content` as added only "if available", but the parser required it, so a
citation without one failed the whole paid search with `malformed web search response:
url_citation.content is missing`, and the answer, the citations that did parse and the cost went
with it. A citation without `content` now carries an empty excerpt. It still needs a `url` and a
`title`, an annotation of another type is still skipped, and a search whose annotations all fail to
parse is still the protocol error it was.

The failure was seen on 2026-11-12 in a live search through `deepseek/deepseek-v4-flash`. The raw
response was not kept, so the one citation that lacked `content` was never inspected. Nine later
searches with the same query all returned five complete citations from one upstream, so the failure
is intermittent and the unseen response is the only evidence for the fix beyond the documented
schema at https://openrouter.ai/docs/features/web-search.
