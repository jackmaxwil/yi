#!/usr/bin/env python3
"""The host's git has one door. Incident: four runners differed in environment, and one honoured a
child's hooks. Anything else spawning git in production source fails here; process.rs holds the door."""
import re, sys, pathlib, tempfile
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, src_files, prod_lines, fail

SPAWN = re.compile(r'\b(?:keyless_)?command\(\s*"git"\s*\)')
DOOR = "crates/tools/src/process.rs"


def offenders(files, root):
    errs = []
    for f in files:
        name = f.relative_to(root).as_posix()
        if name == DOOR:
            continue
        for i, line in enumerate(prod_lines(f), 1):
            if SPAWN.search(line):
                errs.append(f"{name}:{i}: spawn git through yi_tools::host_command: {line.strip()}")
    return errs


def selfcheck():
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        src = root / "crates/x/src"
        src.mkdir(parents=True)
        (root / "crates/tools/src").mkdir(parents=True)
        cases = {
            src / "bad.rs": 'fn a() { let c = yi_tools::command("git"); }\n',
            src / "keyless.rs": 'fn a() { let c = keyless_command("git"); }\n',
            src / "fine.rs": 'fn a() { let c = command("uv"); }\n',
            src / "test_only.rs": '#[cfg(test)]\nmod tests {\n    fn a() { command("git"); }\n}\n',
            root / DOOR: 'fn a() { command("git"); }\n',
        }
        for path, text in cases.items():
            path.write_text(text)
        got = sorted(err.split(":")[0] for err in offenders(cases, root))
        want = ["crates/x/src/bad.rs", "crates/x/src/keyless.rs"]
        fail([] if got == want else [f"selfcheck: {got} != {want}"], "host_git --selfcheck")


if "--selfcheck" in sys.argv:
    selfcheck()
else:
    fail(offenders(src_files(), ROOT), "host_git")
