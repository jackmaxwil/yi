#!/usr/bin/env python3
"""Test LOC has its own ratchet (D31): growth past the fork point's needs `raise: tests +N` in a
change file this branch adds."""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import TESTS, fail, fork, no_fork, texts
from check_changes import raise_errors, raised

base = fork()
if base is None:
    no_fork("test_size")
total = sum(len(t.splitlines()) for t in texts(TESTS).values())
was = sum(len(t.splitlines()) for t in texts(TESTS, base).values())
declared = raised(base, "tests")
fail(raise_errors("tests", total, was, declared), f"test_size ({total}/{was + declared})")
