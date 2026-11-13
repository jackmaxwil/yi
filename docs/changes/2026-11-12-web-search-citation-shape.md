---
issue: Closes #2161
raise: tests +44
---
Web search no longer throws away an answer because one of its citations has no excerpt. OpenRouter
documents `url_citation.content` as added only "if available", but the parser required it, so a
citation without one failed the whole paid search with `malformed web search response:
url_citation.content is missing`, and the answer, the citations that did parse and the cost went
with it. A citation without `content` now carries an empty excerpt. It still needs a `url` and a
`title`, an annotation of another type is still skipped, and a search whose annotations all fail to
parse is still the protocol error it was.

An annotation of type `url_citation` with no nested `url_citation` object, which failed the same
search with `url_citation is missing`, is now read from its own fields. That shape was never seen:
the raw body of the failing response was not kept, so the flat reading is a guess at the field
names, and an annotation that has neither a `url` nor a `title` still fails the search.

The excerpt failure was reproduced on 2026-11-12 by a live search through
`openrouter/deepseek/deepseek-v4.1-flash-1017`. The documented schema is at
https://openrouter.ai/docs/features/web-search.
