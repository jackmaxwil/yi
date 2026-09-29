---
id: necessity
question: should it exist
decider: reader
claim: Why needed
when: {"reads": "full"}
severity: {"high": "no ask, or the change does something the ask did not want", "medium": "scope beyond the ask", "low": "a tangent that could be its own PR"}
refute: {"high": 3, "medium": 1, "low": 1}
---
Judge whether the PR should exist: does "Why needed" name an ask (an issue, the owner's words),
and does the diff answer that ask and nothing beyond it? Changes that answer an earlier review
round's findings are part of the ask. Quote the line of the diff that goes past the ask, or the
"Why needed" line that names none.
