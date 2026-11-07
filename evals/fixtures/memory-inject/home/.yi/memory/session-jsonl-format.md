---
name: session-jsonl-format
description: Sessions are Pi v4 JSONL under ~/.yi/sessions/<cwd mangled>--/<ms>_<id>.jsonl.
type: reference
---
Golden fixtures under crates/types/tests/fixtures round-trip byte for byte.

**How to apply:** read a session's entries line by line; a `custom{memory}` entry names a note, never its text.
