#!/usr/bin/env python3
"""Test LOC has its own ratchet (D31): growth past the fork point's needs `raise: tests +N` in a
change file this branch adds. Inline #[cfg(test)] items in src count too, so moving a test inline
is no way around the raise."""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import SRC, TESTS, fail, fork, no_fork, test_mask, texts
from check_changes import raise_errors, raised


def size(tests, src, off=""):
    inline = 0 if off == "inline" else sum(test_mask(text, off).count(True) for text in src.values())
    return sum(len(text.splitlines()) for text in tests.values()) + inline


def selfcheck(off=""):
    """A bottom `mod tests` and a mid-file helper, shaped like provider.rs's, count as test lines;
    the production fn between them does not, and growing either one needs the raise."""
    lib = "\n".join(["impl R {", "    #[cfg(test)]", "    fn helper(&self) -> u8 {", "        1", "    }",
                     "    pub fn prod(&self) {}", "}", "#[cfg(test)]", "mod tests {", "    #[test]",
                     "    fn t() {}", "}"])
    tests = {"crates/x/tests/a.rs": "fn a() {}\n"}
    errs = []
    if (got := size(tests, {"crates/x/src/lib.rs": lib}, off)) != 1 + 4 + 5:
        errs.append(f"one tests/ line and nine inline test lines measured {got}, not 10")
    grown = lib.replace("    fn t() {}", "    fn t() {}\n    fn u() {}")
    if not raise_errors("tests", size(tests, {"crates/x/src/lib.rs": grown}, off), 10, 0):
        errs.append("an inline test line added without a raise passed")
    return errs


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("inline", "first"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
        fail(errs, "test_size selfcheck")
        sys.exit(0)
    base = fork()
    if base is None:
        no_fork("test_size")
    total, was = size(texts(TESTS), texts(SRC)), size(texts(TESTS, base), texts(SRC, base))
    declared = raised(base, "tests")
    fail(raise_errors("tests", total, was, declared), f"test_size ({total}/{was + declared})")
