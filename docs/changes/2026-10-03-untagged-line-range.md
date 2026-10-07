---
issue: Closes #994
raise: crate runtime +9, crate types +43
---
A `#L<start>-<end>` fragment without `@<tag>` now names the current lines of a `local://` or `checkpoint://` file (Closes #994). A kernel program that knew a function's lines could not hand a child just those lines, because the tag is a hash of content it never read through Yi's tools; the tag stays an optional freshness check and behaves as before when given.

A rollback cost this feature carries: an untagged fragment URL written while it is live
persists verbatim as text wherever a `Url` lands as a string — session JSONL (a child brief
inlines its partition URLs), plan records, output URLs — so replaying that history or
re-resolving those references after reverting fails `Url::parse` until the strings are
re-tagged or hand-edited. Tagged fragments roll back cleanly; untagged ones do not.
