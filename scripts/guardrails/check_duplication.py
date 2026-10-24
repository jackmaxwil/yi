#!/usr/bin/env python3
"""Normalized-window duplication, production = 0: 15 lines of Rust, 6 of prompt or skill prose.
Incidents: 7,779 duplicated blocks in one surveyed harness; another ships its patch grammar in four places."""
import re, sys, pathlib, tempfile
from collections import defaultdict
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, src_files, prod_lines, fail

W = 15
PROSE_W = 6
# A yi:schema block is rendered from the tool schema, so two files listing one shape repeat it by design.
SCHEMA = re.compile(r"<!-- yi:schema .*?<!-- /yi:schema -->", re.S)
PROSE = ("crates/*/src/**/*.md", "skills/**/SKILL.md", "python/skills/**/SKILL.md")


def prose_files(root, off=""):
    return [] if off == "files" else sorted({p for g in PROSE for p in root.glob(g)})


def prose_lines(text, off=""):
    return (text if off == "schema" else SCHEMA.sub("", text)).splitlines()


def duplicates(files, width):
    windows = defaultdict(list)
    for name, lines in files:
        kept = [(n, l.strip()) for n, l in enumerate(lines, 1) if l.strip()]
        for i in range(len(kept) - width + 1):
            windows[tuple(l for _, l in kept[i:i + width])].append(f"{name}:{kept[i][0]}")
    return [f"duplicate {width}-line window: {', '.join(locs[:4])}" for locs in windows.values() if len(locs) > 1]


def prose_errors(root, off=""):
    width = PROSE_W + {"wide": 1, "narrow": -1}.get(off, 0)
    files = [(str(p.relative_to(root)), prose_lines(p.read_text(), off)) for p in prose_files(root, off)]
    return duplicates(files, width)


def selfcheck(off=""):
    """A six-line copy across two skills fails; five lines, and a shared schema block, pass."""
    shared = [f"Shared sentence {n} of the planted copy." for n in range(PROSE_W)]
    block = ["<!-- yi:schema plan /properties/todos/items -->", *[f"- `f{n}` string" for n in range(PROSE_W)],
             "<!-- /yi:schema -->"]
    tree = {
        "skills/yi/a/SKILL.md": ["# a", *shared, "a ends"],
        "python/skills/b/SKILL.md": ["# b", "", *shared, "b ends"],
        "skills/yi/c/SKILL.md": ["# c", *shared[:-1], "c ends"],
        "skills/yi/d/SKILL.md": ["# d", *shared[:-1], "d ends"],
        "crates/x/src/p.md": ["p starts", *block, "p ends"],
        "crates/x/src/prompts/q.md": ["q starts", *block, "q ends"],
    }
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        for name, lines in tree.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text("\n".join(lines) + "\n")
        errs = prose_errors(root, off)
    want = [f"duplicate {PROSE_W}-line window: python/skills/b/SKILL.md:3, skills/yi/a/SKILL.md:2"]
    return [] if errs == want else [f"prose scan said {errs}, not {want}"]


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("files", "schema", "wide", "narrow"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
        fail(errs, "duplication selfcheck")
        sys.exit(0)
    rust = [(str(f.relative_to(ROOT)), prod_lines(f)) for f in src_files()]
    fail(duplicates(rust, W) + prose_errors(ROOT), "duplication")
