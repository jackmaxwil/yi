---
version: 0.910.0
issue: Closes #1555
raise: tests +60
---
The document converter that `read` and `grep` run in the sandboxed venv python is now `python/yi_runtime/src/yi/convert.py` instead of a 191-line raw string in `crates/tools/src/document.rs`. `document.rs` carries that file with `include_str!` and spawns `python -I -c <the program> <source> <staging> <pages>` — the same sandbox, stdin bytes and one-line JSON reply, so every caller sees the same results. The program is carried rather than resolved: a venv whose installed `yi` package predates the binary converts anyway, where `-m yi.convert` there would fail with `No module named yi.convert`, and `-c` skips the package `__init__` that would otherwise import `rlm` and `ipykernel` on every uncached conversion. `anydoc` and `pdf_inspector` are imported by the function that uses them, so importing the module needs neither and the stdlib unittest lane tests the PDF running-header removal and the spreadsheet tidying against markdown recorded from the real producers (`python/yi_runtime/tests/vectors/convert.json`). The change is a kernel-package change: it moves the runtime identity hash, so the venv is rebuilt on next boot.
