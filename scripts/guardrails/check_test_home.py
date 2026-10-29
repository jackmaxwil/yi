#!/usr/bin/env python3
"""Incident: 11,633 of the 11,635 checkpoint dirs under the owner's real ~/.yi came from test
runs that spawned the binary without a HOME of their own. The binary's path is named once, in
the helper that pins HOME, so a new spawn cannot forget."""
import sys, pathlib

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, TESTS, fail

NAME = 'CARGO_BIN_EXE_yi'
HELPER = 'crates/cli/tests/support/yi_bin.rs'


def strays(files, root=ROOT):
    return [f"{f.relative_to(root)}: names {NAME} outside {HELPER}"
            for f in files
            if f.relative_to(root).as_posix() != HELPER and NAME in f.read_text()]


def test_files(root):
    return sorted(p for p in (root / "crates").glob("*/tests/**/*.rs") if TESTS.match(p.relative_to(root).as_posix()))


def selfcheck():
    bad = []
    import tempfile
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        helper = root / HELPER
        helper.parent.mkdir(parents=True)
        helper.write_text(f"pub const BIN: &str = env!(\"{NAME}\");\n")
        stray = root / "crates/cli/tests/stray.rs"
        stray.write_text(f"let c = Command::new(env!(\"{NAME}\"));\n")
        if not strays([helper, stray], root):
            bad.append("a test file naming the binary beside the helper is not refused")
        if strays([helper], root):
            bad.append("the helper itself is refused")
        outside = root / "crates/cli/src/lib.rs"
        outside.parent.mkdir(parents=True)
        outside.write_text(f"const B: &str = env!(\"{NAME}\");\n")
        if outside in test_files(root):
            bad.append("a src file, which no test spawns from, is read as a test file")
    return bad


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        fail(errs, "test_home selfcheck")
        sys.exit(0)
    fail(strays(test_files(ROOT)), "test_home")
