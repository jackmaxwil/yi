#!/usr/bin/env python3
"""Incident: 11,633 of the 11,635 checkpoint dirs under the owner's real ~/.yi came from test
runs that spawned the binary without a HOME of their own. The binary's path is named once, in
the helper that pins HOME, so a new spawn cannot forget."""
import sys, pathlib

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, TESTS, fail

NAME = 'CARGO_BIN_EXE_yi'
HELPER = 'crates/cli/tests/support/yi_bin.rs'
BARE = 'yi_bin::bin('
BARE_OK = {'crates/cli/tests/tui_drive.rs', 'crates/cli/tests/monkey.rs', 'crates/cli/tests/browser.rs',
           'crates/cli/tests/cli_surfaces.rs'}


def strays(files, root=ROOT):
    out = []
    for f in files:
        rel = f.relative_to(root).as_posix()
        text = f.read_text()
        if rel != HELPER and NAME in text:
            out.append(f"{rel}: names {NAME} outside {HELPER}")
        if rel != HELPER and rel not in BARE_OK and BARE in text:
            out.append(f"{rel}: takes the bare binary path ({BARE}) without a HOME; use yi_command")
    return out


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
        bare = root / "crates/cli/tests/bare.rs"
        bare.write_text("Command::new(crate::yi_bin::bin());\n")
        if not strays([bare], root):
            bad.append("a bare yi_bin::bin( spawn outside the allowlist is not refused")
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
