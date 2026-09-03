#!/usr/bin/env python3
"""Write-only `pub` fields and reader-less baselines, both at zero (D109). rustc's dead_code
sees nothing `pub`, so a field that crosses a crate boundary and is never read again warns
nowhere; two context budgets were defaulted and enforced by no one, and a 0-byte baseline
added at 0.97.0 went unread for 26 versions until this scan named it. crates/types is
exempt (the wire reads it) and so is any struct deriving Serialize/Deserialize; a test
counts as a reader."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail, prod_lines

STRUCT = re.compile(r"\bstruct\s+\w+")
FIELD = re.compile(r"^\s*pub\s+(\w+)\s*:")
SERDE = re.compile(r"\bSerialize\b|\bDeserialize\b")
READ = re.compile(r"\.(\w+)\b(?!\s*\()(?!\s*=[^=])")
BOUND = re.compile(r"[{,]\s*(\w+)\s*(?=[,}])")
CORPUS = ("scripts", "evals", "skills")


def declared(text):
    """(name, line) per `pub name:` in a struct body, skipping serde-derived structs."""
    out, depth, exempt, attrs = [], 0, False, ""
    for i, line in enumerate(text.splitlines(), 1):
        if depth:
            m = FIELD.match(line)
            if m and depth == 1 and not exempt:
                out.append((m.group(1), i))
            depth += line.count("{") - line.count("}")
        elif STRUCT.search(line) and "{" in line:
            depth = line.count("{") - line.count("}")
            exempt, attrs = bool(SERDE.search(attrs)), ""
        elif line.lstrip().startswith("#["):
            attrs += line
        else:
            attrs = ""
    return out


def read_names(texts):
    """Names read as `.name` (not a call, not an assignment) or bound in `{ name, }`."""
    names = set()
    for text in texts:
        names |= set(READ.findall(text)) | set(BOUND.findall(text))
    return names


def baseline_orphans(names, corpus):
    """Baselines whose filename appears in no corpus file but their own."""
    return [
        n
        for n in names
        if not any(n in t for p, t in corpus if not p.endswith(f"baselines/{n}"))
    ]


SELF_SRC = """#[derive(Debug)]
pub struct A {
    pub kept: u8,
    pub dropped: u8,
}

#[derive(Serialize, Deserialize)]
pub struct B {
    pub wire: u8,
}
"""
SELF_READERS = ["fn f(a: &A) -> u8 { a.kept }"]
SELF_BASELINES = ["read_me.json", "nobody.json"]
SELF_CORPUS = [
    ("scripts/gate.py", 'BASE / "read_me.json"'),
    ("scripts/guardrails/baselines/nobody.json", "{}"),
]


def selfcheck(off=""):
    bad = []
    fields = (
        []
        if off == "fields"
        else [n for n, _ in declared(SELF_SRC) if n not in read_names(SELF_READERS)]
    )
    if fields != ["dropped"]:
        bad.append(f"field scan said {fields}, not ['dropped']: it must flag the written-and-never-read field, spare the read one, and exempt the serde-derived one")
    stale = [] if off == "baselines" else baseline_orphans(SELF_BASELINES, SELF_CORPUS)
    if stale != ["nobody.json"]:
        bad.append(f"baseline scan said {stale}, not ['nobody.json']: it must flag the baseline no file but itself names, and spare the one a script opens")
    return bad


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("fields", "baselines"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} scan disabled, so it refutes nothing")
        fail(errs, "orphans selfcheck")
        sys.exit(0)

    src = sorted((ROOT / "crates").glob("*/src/**/*.rs"))
    reads = read_names(f.read_text() for f in src + sorted((ROOT / "crates").glob("*/tests/**/*.rs")))
    fields = [
        (f.relative_to(ROOT), name, line)
        for f in src
        if f.relative_to(ROOT).parts[1] != "types"
        for name, line in declared("\n".join(prod_lines(f)))
        if name not in reads
    ]
    corpus = [(str(p.relative_to(ROOT)), p.read_text(errors="ignore")) for p in
              sorted(q for d in CORPUS for q in (ROOT / d).rglob("*") if q.is_file())]
    corpus.append(("justfile", (ROOT / "justfile").read_text()))
    names = sorted(p.name for p in BASE.iterdir() if p.is_file())
    errs = [f"{f}:{line}: pub {name} is written and never read" for f, name, line in fields]
    errs += [f"baselines/{n}: no script, eval, skill or justfile line names it" for n in baseline_orphans(names, corpus)]
    fail(errs, f"orphans (0 write-only fields, 0 reader-less of {len(names)} baselines)")
