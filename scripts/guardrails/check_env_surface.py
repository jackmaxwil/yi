#!/usr/bin/env python3
"""Every YI_* env var is a declared row; cap 40. Incident: 517 prefixed vars, 87%
undocumented. Read wherever a name can be set or read: src, tests, scripts, workflows and the
justfile, since a scan of src alone left seven names in use undeclared. A row nothing reads fails."""
import json, re, sys, pathlib, tempfile
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

CAP = 40
SURFACES = {
    "src": ("crates/*/src/**/*.rs", "python/yi_runtime/src/**/*.py"),
    "tests": ("crates/*/tests/**/*.rs", "python/yi_runtime/tests/**/*.py"),
    "scripts": ("scripts/**/*.py", "scripts/**/*.sh", "scripts/hooks/*"),
    "workflows": (".forgejo/workflows/*.yml",),
    "justfile": ("justfile",),
}
# A name inside a longer one (ARC_YI_MODEL) is that harness's own, and YI_DESIGN.md is a file.
NAME = re.compile(r"(?<![A-Za-z0-9_])YI_[A-Z][A-Z0-9_]*(?![A-Za-z0-9_]|\.md)")
LOOSE = re.compile(r"YI_[A-Z_]+")


def used(root, off=""):
    pattern = LOOSE if off == "boundary" else NAME
    found = {}
    for surface, globs in SURFACES.items():
        if off == surface:
            continue
        for path in sorted({p for g in globs for p in root.glob(g) if p.is_file()}):
            if path.resolve() == pathlib.Path(__file__).resolve() or "fixtures" in path.parts:
                continue
            for name in pattern.findall(path.read_text(errors="replace")):
                found.setdefault(name, str(path.relative_to(root)))
    return found


def errors(root, declared, off=""):
    found = used(root, off)
    errs = [f"undeclared env var {v} in {found[v]} (add to baselines/env_vars.json)" for v in sorted(set(found) - declared)]
    if off != "unused":
        errs += [f"declared env var {v} is read nowhere (drop it from baselines/env_vars.json)" for v in sorted(declared - set(found))]
    if len(declared) > CAP and off != "cap":
        errs.append(f"env surface {len(declared)} > hard cap {CAP}")
    return errs, found


def selfcheck(off=""):
    """One name per surface goes undeclared, a declared name nothing reads fails, the cap fires one
    past 40, and neither a harness's ARC_YI_ name nor YI_DESIGN.md counts."""
    tree = {
        "crates/x/src/a.rs": 'std::env::var("YI_SRC_VAR")',
        "crates/x/tests/orient.rs": '.env("YI_FAKE_RIPWIRE", dir) // see docs/YI_DESIGN.md',
        "python/yi_runtime/src/yi/example.py": 'os.environ["YI_RUNTIME_ONLY"]',
        "scripts/run.sh": 'key="${YI_SIGNING_KEY:-x}" ARC_YI_MODEL=m',
        ".forgejo/workflows/review.yml": "env:\n  YI_ROUND_AUTHORS: bot\n",
        "justfile": "bless:\n    YI_BLESS=1 cargo test\n",
    }
    names = {"YI_SRC_VAR", "YI_FAKE_RIPWIRE", "YI_SIGNING_KEY", "YI_ROUND_AUTHORS", "YI_BLESS", "YI_RUNTIME_ONLY"}
    errs = []
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        for name, text in tree.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text(text)
        found = errors(root, set(), off)[0]
        if len(found) != len(names) or not all(any(n in e for e in found) for n in names):
            errs.append(f"nothing declared: gate said {found}, not one undeclared line per surface")
        found = errors(root, names | {"YI_UNUSED"}, off)[0]
        if found != ["declared env var YI_UNUSED is read nowhere (drop it from baselines/env_vars.json)"]:
            errs.append(f"one unread row: gate said {found}")
        padded = {f"YI_PAD_{n:02}" for n in range(CAP - len(names))}
        for extra, want in ((set(), []), ({"YI_PAD_XX"}, [f"env surface {CAP + 1} > hard cap {CAP}"])):
            found = [e for e in errors(root, names | padded | extra, off)[0] if "read nowhere" not in e]
            if found != want:
                errs.append(f"{len(names | padded | extra)} declared: gate said {found}, not {want}")
    return errs


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in (*SURFACES, "unused", "cap", "boundary"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
        fail(errs, "env_surface selfcheck")
        sys.exit(0)
    declared = set(json.loads((BASE / "env_vars.json").read_text()))
    errs, found = errors(ROOT, declared)
    fail(errs, f"env_surface ({len(found)} used / {len(declared)} declared)")
