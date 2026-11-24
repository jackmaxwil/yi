#!/usr/bin/env python3
"""The HAR audit's inventories: every site of each kind, outside tests unless said.

Usage: inventory.py [KIND ...]. Prints one markdown table per kind; docs/har-audit.md
holds the verdicts. Kinds: allow, unsafe, spawn, clock, random, thread, lock, write.
"""

import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
from har import ATTR, ROOT, code_lines, strip_tests  # noqa: E402

KINDS = {
    "unsafe": r"\bunsafe\s*(\{|impl|fn|extern)",
    "spawn": r"Command::new\(",
    "clock": r"\b(Instant|SystemTime)::now\(",
    "random": r"\b(RandomState|getrandom|rand::|thread_rng|HashMap::new|HashSet::new|HashMap<|HashSet<)",
    "thread": r"(thread::(spawn|scope)\(|Builder::new\(\))",
    "lock": r"\b(Mutex|RwLock|Condvar|Atomic(Bool|U\w+|I\w+|Ptr))(::new|<)",
    "write": r"(fs::write|File::create|OpenOptions::new|fs::rename|fs::remove_file|fs::create_dir|fs::copy|\.sync_all\(|\.sync_data\()",
}


def sources():
    """(relative path, text with test modules blanked, is a test file) for every crate file."""
    files = sorted(ROOT.glob("crates/*/**/*.rs"))
    tests = set()
    for f in files:
        _, mods = strip_tests(f.read_text())
        base = f.parent if f.name in ("lib.rs", "main.rs", "mod.rs") else f.with_suffix("")
        tests |= {base / f"{m}.rs" for m in mods} | {base / m / "mod.rs" for m in mods}
    for f in files:
        rel = f.relative_to(ROOT)
        in_test = "tests" in rel.parts or rel.name == "tests.rs" or f in tests
        yield str(rel), f.read_text(), in_test


def allows():
    rows = []
    for rel, text, in_test in sources():
        live = strip_tests(text)[0].splitlines()
        for m in ATTR.finditer(text):
            n = text.count("\n", 0, m.start()) + 1
            test = in_test or n > len(live) or not live[n - 1].strip()
            rows.append((rel, n, "test" if test else "src", " ".join(m.group(0).split())))
    return rows


def sites(kind):
    pattern = re.compile(KINDS[kind])
    rows = []
    for rel, text, in_test in sources():
        if in_test:
            continue
        for n, line in code_lines(strip_tests(text)[0]):
            if pattern.search(line):
                rows.append((rel, n, "src", line.strip()))
    return rows


def main():
    kinds = sys.argv[1:] or ["allow", *KINDS]
    for kind in kinds:
        rows = allows() if kind == "allow" else sites(kind)
        print(f"\n### {kind}: {len(rows)}\n\n| Site | Where | Code |\n|---|---|---|")
        for rel, n, where, code in rows:
            print(f"| `{rel}:{n}` | {where} | `{code.replace('|', '¦')}` |")
    return 0


if __name__ == "__main__":
    sys.exit(main())
