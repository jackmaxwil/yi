#!/usr/bin/env python3
"""Write-only fields, dead shapes and reader-less baselines, all at zero (D109). rustc's dead_code
sees nothing `pub`, so a field that crosses a crate boundary and is never read again warns
nowhere. A read is `.name` in production code, or the name bound by a destructuring pattern; a
struct literal that sets a field reads nothing, and a test is not a reader. Enum variant fields
are scanned like struct fields. crates/types fields are exempt (the wire reads them) and so is
anything deriving Serialize/Deserialize, but a `pub struct` or `pub enum` there whose name
appears in no production code outside its own definition is dead."""
import re, sys, pathlib
from collections import Counter
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail, item_end, normal_code, prod_lines, src_files

ITEM = re.compile(r"((?:[ \t]*#\[[^\]]*\]\s*)*)[ \t]*(?:pub(?:\([^)]*\))?[ \t]+)?(struct|enum)[ \t]+\w+[^{;]*?\{")
SHAPE = re.compile(r"\bpub\s+(?:struct|enum)\s+(\w+)")
PUB_FIELD = re.compile(r"(?m)(?:^|,)[ \t]*pub[ \t]+(\w+)\s*:(?!:)")
VARIANT_FIELD = re.compile(r"(?:^|[{,])\s*(\w+)\s*:(?!:)")
VARIANT = re.compile(r"\b[A-Z]\w*\s*\{")
SERDE = re.compile(r"\bSerialize\b|\bDeserialize\b")
READ = re.compile(r"\.(\w+)\b(?!\s*\()(?!\s*=[^=])")
LITERAL = re.compile(r"\b(?:\w+::)*[A-Z]\w*\s*\{")
NOT_LITERAL = re.compile(r"(?:->|\b(?:struct|enum|impl|for|trait|union|dyn|mod))\s*$")
PATTERN_BEFORE = re.compile(r"\blet\s+(?:ref\s+|mut\s+)?$|\bmatches!\([^;]*,\s*$")
PATTERN_AFTER = re.compile(r"[)\]}\s,]*(?:=>|=(?!=)|\|(?!\|)|:(?!:)|if\b)")
REST_PATTERN = re.compile(r"\.\.\s*(?:,|$)")
BOUND = re.compile(r"(?:^|[{,])\s*(?:ref\s+|mut\s+)*(\w+)\s*(?=,|$|:(?!:))")
CORPUS = ("scripts", "evals", "skills")


def groups(text):
    """(open, close) offsets of each outermost brace pair in already-normalised code."""
    at = text.find("{")
    while at >= 0:
        close = item_end(text, at)
        yield at, close
        at = text.find("{", close)


def declared(text, enums=True):
    """(name, line) per field of a struct (`pub name:`) or enum variant (`name:`) in normalised
    production code, skipping serde-derived items."""
    out = []
    for m in ITEM.finditer(text):
        if SERDE.search(m.group(1)) or (m.group(2) == "enum" and not enums):
            continue
        body = text[m.end():item_end(text, m.end() - 1)]
        if m.group(2) == "struct":
            flat = "".join(c if c != "{" else " " for c in body)
            found = [(f.group(1), f.start(1)) for f in PUB_FIELD.finditer(flat)]
        else:
            found = [(f.group(1), o + 1 + f.start(1)) for o, c in groups(body) for f in VARIANT_FIELD.finditer(body[o + 1:c])]
        out += [(name, text.count("\n", 0, m.end() + at) + 1) for name, at in found]
    return out


def bound_names(text):
    """Field names a destructuring pattern in the text binds. A brace group is a pattern, not a
    literal, when it ends in `..`, sits after `let` or inside `matches!`, or is followed by `=>`,
    `=`, `|` or `if`."""
    names = set()
    for m in LITERAL.finditer(text):
        if NOT_LITERAL.search(text[max(0, m.start() - 12):m.start()]):
            continue
        close = item_end(text, m.end() - 1)
        content = text[m.end():close]
        if (REST_PATTERN.search(content) or PATTERN_BEFORE.search(text[max(0, m.start() - 80):m.start()])
                or PATTERN_AFTER.match(text[close + 1:close + 40])):
            names |= set(BOUND.findall(content))
    return names


def interpolated(raw):
    """Names a thiserror `#[error("...{name}...")]` message interpolates: the derive reads those fields."""
    return {n for attr in re.findall(r"#\[error\((.*?)\)\]", raw, re.S) for n in re.findall(r"\{(\w+)[^{}]*\}", attr)}


def read_names(texts):
    """Names read as `.name` (not a call, not an assignment) or bound by a destructuring pattern."""
    names = set()
    for text in texts:
        names |= set(READ.findall(text)) | bound_names(text)
    return names


def dead_shapes(code):
    """yi-types `pub struct`/`pub enum` names that no production code names outside their own item.
    `code` maps repo path to normalised production code."""
    tokens = Counter(t for text in code.values() for t in re.findall(r"\w+", text))
    out = []
    for path, text in code.items():
        if path.split("/")[1] != "types":
            continue
        for m in SHAPE.finditer(text):
            own = Counter(re.findall(r"\w+", text[m.start():item_end(text, m.end())]))
            if tokens[m.group(1)] == own[m.group(1)]:
                out.append((path, m.group(1), text.count("\n", 0, m.start()) + 1))
    return out


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
    pub recorded_at: u64,
}

pub enum Edit {
    Delete { line: u8, old_assertion: String },
}

#[derive(Serialize, Deserialize)]
pub struct B {
    pub wire: u8,
}
"""
SELF_READERS = [
    "fn f(a: &A) -> u8 { a.kept }",
    "fn g(recorded_at: u64) -> A { A { kept: 0, dropped: 1, recorded_at } }",
    "fn h(e: Edit) -> u8 { match e { Edit::Delete { line, .. } => line } }",
    "fn k() -> Edit { let line = 1; Edit::Delete { line, old_assertion: String::new() } }",
    "fn m(A { dropped, .. }: A) -> u8 { dropped }",
]
SELF_FLAGGED = ["old_assertion", "recorded_at"]
SELF_SHAPES = {
    "crates/types/src/tape.rs": "pub struct Tape {\n    pub rows: u8,\n}\npub struct Used {\n    pub n: u8,\n}\n",
    "crates/runtime/src/use.rs": "fn f(u: Used) -> u8 { u.n }",
}
SELF_BASELINES = ["read_me.json", "nobody.json"]
SELF_CORPUS = [
    ("scripts/gate.py", 'BASE / "read_me.json"'),
    ("scripts/guardrails/baselines/nobody.json", "{}"),
]
LOOSE_BOUND = re.compile(r"[{,]\s*(\w+)\s*(?=[,}])")


def selfcheck(off=""):
    bad = []
    reads = read_names(SELF_READERS)
    if off == "literals":
        reads = reads.union(*(set(LOOSE_BOUND.findall(t)) for t in SELF_READERS))
    flagged = [] if off == "fields" else sorted(n for n, _ in declared(SELF_SRC, enums=off != "variants") if n not in reads)
    if flagged != sorted(SELF_FLAGGED):
        bad.append(f"field scan said {flagged}, not {SELF_FLAGGED}: it must flag a field only a struct literal or a variant literal sets, spare the one read as `.name` or bound by a destructuring pattern, and exempt the serde-derived one")
    dead = [] if off == "shapes" else [n for _, n, _ in dead_shapes(SELF_SHAPES)]
    if dead != ["Tape"]:
        bad.append(f"shape scan said {dead}, not ['Tape']: it must flag the yi-types shape named nowhere else and spare the one production code names")
    stale = [] if off == "baselines" else baseline_orphans(SELF_BASELINES, SELF_CORPUS)
    if stale != ["nobody.json"]:
        bad.append(f"baseline scan said {stale}, not ['nobody.json']: it must flag the baseline no file but itself names, and spare the one a script opens")
    return bad


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("fields", "literals", "variants", "shapes", "baselines"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} scan disabled, so it refutes nothing")
        fail(errs, "orphans selfcheck")
        sys.exit(0)

    raw = {str(f.relative_to(ROOT)): "\n".join(prod_lines(f)) for f in src_files()}
    code = {path: normal_code(text) for path, text in raw.items()}
    reads = read_names(code.values()).union(*map(interpolated, raw.values()))
    fields = [
        (path, name, line)
        for path, text in code.items()
        if path.split("/")[1] != "types"
        for name, line in declared(text)
        if name not in reads
    ]
    corpus = [(str(p.relative_to(ROOT)), p.read_text(errors="ignore")) for p in
              sorted(q for d in CORPUS for q in (ROOT / d).rglob("*")
                     if q.is_file() and "__pycache__" not in q.parts)]
    corpus.append(("justfile", (ROOT / "justfile").read_text()))
    names = sorted(p.name for p in BASE.iterdir() if p.is_file())
    errs = [f"{f}:{line}: pub {name} is written and never read" for f, name, line in fields]
    errs += [f"{f}:{line}: pub {name} is named nowhere in production code outside its own definition" for f, name, line in dead_shapes(code)]
    errs += [f"baselines/{n}: no script, eval, skill or justfile line names it" for n in baseline_orphans(names, corpus)]
    fail(errs, f"orphans (0 write-only fields, 0 dead shapes, 0 reader-less of {len(names)} baselines)")
