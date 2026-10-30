---
issue: Closes #1555
---
The document converter that `read` and `grep` run in the sandboxed venv python is now `python/yi_runtime/src/yi/convert.py` instead of a 191-line raw string in `crates/tools/src/document.rs`. `document.rs` spawns `python -I -m yi.convert <source> <staging> <pages>` with the same sandbox, stdin bytes and one-line JSON reply, so every caller sees the same results. `anydoc` and `pdf_inspector` are imported only by `main`, which lets the stdlib unittest lane test the PDF running-header removal and the spreadsheet tidying against markdown recorded from the real producers (`python/yi_runtime/tests/vectors/convert.json`). The change is a kernel-package change: it moves the runtime identity hash, so the venv is rebuilt on next boot.
