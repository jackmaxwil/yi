#!/usr/bin/env python3
"""Normalized-window duplication, production = 0: 15 lines of Rust, 6 of prompt or skill prose.
Incidents: 7,779 duplicated blocks in one surveyed harness; another ships its patch grammar in four places.
Tests may not grow their duplicated windows past the fork's: 107 of them sat in no budget, so a
copied test was free."""
import re, sys, pathlib, tempfile
from collections import defaultdict
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, TESTS, code_lines, fail, fork, no_fork, texts

W = 15
PROSE_W = 6
# A yi:schema block is rendered from the tool schema, so two files listing one shape repeat it by design.
SCHEMA = re.compile(r"<!-- yi:schema .*?<!-- /yi:schema -->", re.S)
WIDEN = {"wide": 1, "narrow": -1}
PROSE = ("crates/*/src/**/*.md", "skills/**/SKILL.md", "python/skills/**/SKILL.md")


def rust_errors(root, off=""):
    files = [] if off == "rust" else sorted(root.glob("crates/*/src/**/*.rs"))
    cut = str.splitlines if off == "cfg" else lambda text: code_lines(text, off)
    return duplicates([(str(p.relative_to(root)), cut(p.read_text())) for p in files], W + WIDEN.get(off, 0))


def prose_files(root, off=""):
    return [] if off == "files" else sorted({p for g in PROSE for p in root.glob(g)})


def prose_lines(text, off=""):
    # A block is blanked, not deleted, so a hit below it names its line in the file.
    return (text if off == "schema" else SCHEMA.sub(lambda m: "\n" * m.group().count("\n"), text)).splitlines()


def windows(files, width):
    found = defaultdict(list)
    for name, lines in files:
        kept = [(n, l.strip()) for n, l in enumerate(lines, 1) if l.strip()]
        for i in range(len(kept) - width + 1):
            found[tuple(l for _, l in kept[i:i + width])].append(f"{name}:{kept[i][0]}")
    return found


def duplicates(files, width):
    return [f"duplicate {width}-line window: {', '.join(locs[:4])}" for locs in windows(files, width).values() if len(locs) > 1]


def test_errors(now, was, off=""):
    """A window copied more often in tests than at the fork; `now` and `was` map path to text."""
    if off == "tests":
        return []
    cut = lambda files: windows(sorted((name, text.splitlines()) for name, text in files.items()), W)
    old = {} if off == "fork" else cut(was)
    return [f"duplicate {W}-line test window: {', '.join(locs[:4])} ({len(locs)} copies, the fork had {len(old.get(key, []))})"
            for key, locs in cut(now).items() if len(locs) > max(len(old.get(key, [])), 1)]


def prose_errors(root, off=""):
    width = PROSE_W + WIDEN.get(off, 0)
    files = [(str(p.relative_to(root)), prose_lines(p.read_text(), off)) for p in prose_files(root, off)]
    return duplicates(files, width)


def errors(root, off=""):
    return rust_errors(root, off) + prose_errors(root, off)


def selfcheck(off=""):
    """A copy at each window fails at its line in the file, below a mid-file test helper too; one
    line shorter, test code under #[cfg(test)] and a shared schema block pass. In tests a copy the
    fork already had passes and a new one fails."""
    shared = [f"Shared sentence {n} of the planted copy." for n in range(6)]
    block = ["<!-- yi:schema plan /properties/todos/items -->", *[f"- `f{n}` string" for n in range(6)],
             "<!-- /yi:schema -->"]
    code = [f"let v{n} = {n};" for n in range(15)]
    tests = ["#[cfg(test)]", "mod tests {", *[f"let t{n} = {n};" for n in range(15)], "}"]
    helper = ["impl R {", "    #[cfg(test)]", "    fn helper(&self) -> u8 {", "        1", "    }", "    fn prod() {"]
    tree = {
        "skills/yi/a/SKILL.md": ["# a", *block, *shared, "a ends"],
        "python/skills/b/SKILL.md": ["# b", "", *shared, "b ends"],
        "skills/yi/c/SKILL.md": ["# c", *shared[:-1], "c ends"],
        "skills/yi/d/SKILL.md": ["# d", *shared[:-1], "d ends"],
        "crates/x/src/p.md": ["p starts", *block, "p ends"],
        "crates/x/src/prompts/q.md": ["q starts", *block, "q ends"],
        "crates/x/src/a.rs": ["fn a() {", *code, "}", *tests],
        "crates/y/src/b.rs": ["fn b() {", "", *code, "} // b", *tests],
        "crates/z/src/c.rs": ["fn c() {", *code[:-1], "} // c", *tests[:1]],
        "crates/w/src/d.rs": [*helper, *code, "    } // prod", "}"],
    }
    with tempfile.TemporaryDirectory() as tmp:
        root = pathlib.Path(tmp)
        for name, lines in tree.items():
            (root / name).parent.mkdir(parents=True, exist_ok=True)
            (root / name).write_text("\n".join(lines) + "\n")
        errs = errors(root, off)
    old, new = [f"old line {n}" for n in range(15)], [f"new line {n}" for n in range(15)]
    was = {"crates/x/tests/a.rs": "\n".join(old), "crates/x/tests/b.rs": "\n".join(["// b", *old, *new])}
    errs += test_errors({**was, "crates/x/tests/c.rs": "\n".join(new)}, was, off)
    want = ["duplicate 15-line window: crates/w/src/d.rs:7, crates/x/src/a.rs:2, crates/y/src/b.rs:3",
            "duplicate 6-line window: python/skills/b/SKILL.md:3, skills/yi/a/SKILL.md:10",
            "duplicate 15-line test window: crates/x/tests/b.rs:17, crates/x/tests/c.rs:1 (2 copies, the fork had 1)"]
    return [] if errs == want else [f"scan said {errs}, not {want}"]


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("rust", "cfg", "first", "files", "schema", "wide", "narrow", "tests", "fork"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
        fail(errs, "duplication selfcheck")
        sys.exit(0)
    base = fork()
    if base is None:
        no_fork("duplication")
    fail(errors(ROOT) + test_errors(texts(TESTS), texts(TESTS, base)), "duplication")
