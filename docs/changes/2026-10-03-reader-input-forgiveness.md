---
issue: Closes #989, Closes #993
raise: crate runtime +10, crate tools +28, tests +54, comments +4
---
A reader's JSON answer is taken from a fence after prose (Closes #989). `rlm.result(schema=...)` refused a reply that was a paragraph followed by one fenced object matching the schema, as "answered with text, not JSON", because only a reply that began with a fence was unwrapped. When the reply does not parse whole, the last fenced block that parses as JSON is the answer, and the schema checks it as before; a reply with no parseable JSON is refused as before.

`read` takes the `path#TAG` form that grep prints (Closes #993). A reader passed `more_itertools/more.py#F0C2` and got "No such file or directory". When the path as given does not exist and the path without a trailing `#` and four hex digits is a file, `read` reads that file, through the same header parser `edit` uses; a file really named with a tag still reads. A tag that is not the file's current version is named at the top of the result, with the file's current tag, and the current content follows.
