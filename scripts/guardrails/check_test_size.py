#!/usr/bin/env python3
"""Test LOC has its own ratchet (D31): growth past the fork point's needs `raise: tests +N` in a
change file this branch adds."""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import TESTS, fail, fork, no_fork, texts
from check_changes import raised

base = fork()
if base is None:
    no_fork("test_size")
total = sum(len(t.splitlines()) for t in texts(TESTS).values())
was = sum(len(t.splitlines()) for t in texts(TESTS, base).values())
limit = was + raised(base, "tests")
errs = [f"test LOC {total} > {limit} (fork {was}); a change file here says `raise: tests +{total - was}`"] if total > limit else []
fail(errs, f"test_size ({total}/{limit})")
