---
issue: Closes #994
raise: crate runtime +9, crate types +43
---
A `#L<start>-<end>` fragment without `@<tag>` now names the current lines of a `local://` or `checkpoint://` file (Closes #994). A kernel program that knew a function's lines could not hand a child just those lines, because the tag is a hash of content it never read through Yi's tools; the tag stays an optional freshness check and behaves as before when given.
