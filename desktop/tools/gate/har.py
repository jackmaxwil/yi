#!/usr/bin/env python3
"""HAR gate: the crate-root, manifest and source rules clippy alone cannot hold.

Usage: har.py [--selfcheck]. Exit 0 when clean, 1 with one line per problem.
"""

import re
import sys
import tomllib
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
REQUIRED_DENY = ["unwrap_used", "expect_used", "panic", "todo", "unreachable",
                 "unimplemented", "await_holding_lock", "disallowed_methods"]
DENIED = set(REQUIRED_DENY) | {"await_holding_invalid_type", "string_slice", "unsafe_code"}
PANICKY = re.compile(r"\.unwrap\(\)|\.expect\(|\b(?:panic|todo|unimplemented|unreachable)!")
# print! and eprintln! panic when the stream is closed: an app started with stderr shut.
PRINT = re.compile(r"\be?print(?:ln)?!")
ATTR = re.compile(r"#!?\[\s*(allow|expect)\s*\((.*?)\)\s*\]", re.S)
GLOB = re.compile(r"\bpub(?:\([^)]*\))?\s+use\b[^;]*\*[^;]*;", re.S)
UNSAFE_BLOCK = re.compile(r"\bunsafe\s*(?:\{|impl\b)")
CFG_TEST = re.compile(r"#\[cfg\(test\)\]\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+(\w+)\s*([;{])")


def is_mac(rel):
    return rel.startswith("crates/mac/")


def code_lines(text):
    """(line number, line) for every line that is not a // comment."""
    return [(n, l) for n, l in enumerate(text.splitlines(), 1) if not l.lstrip().startswith("//")]


def strip_tests(text):
    """Text with inline #[cfg(test)] modules blanked; names of out-of-line test modules."""
    out, files, pos = [], [], 0
    for m in CFG_TEST.finditer(text):
        if m.start() < pos:
            continue
        if m.group(2) == ";":
            files.append(m.group(1))
            continue
        depth, i = 0, m.end() - 1
        while i < len(text):
            depth += {"{": 1, "}": -1}.get(text[i], 0)
            if depth == 0:
                break
            i += 1
        out.append(text[pos:m.start()])
        out.append("\n" * text.count("\n", m.start(), i + 1))
        pos = i + 1
    out.append(text[pos:])
    return "".join(out), files


MAC_LINTS = ("clippy::undocumented_unsafe_blocks", "clippy::multiple_unsafe_ops_per_block")


def check_root(rel, text):
    unsafe_attr = "#![deny(unsafe_code)]" if is_mac(rel) else "#![forbid(unsafe_code)]"
    problems = [f"{rel}: crate root lacks {a}" for a in (unsafe_attr, "#![deny(clippy::string_slice)]")
                if a not in text]
    denied = " ".join(m.group(1) for m in re.finditer(r"#!\[\s*deny\s*\((.*?)\)\s*\]", text, re.S))
    return problems + [f"{rel}: crate root does not deny {lint}" for lint in MAC_LINTS
                       if is_mac(rel) and lint not in denied]


def check_manifest(rel, text):
    lints = tomllib.loads(text).get("lints", {})
    return [] if lints.get("workspace") is True else [f"{rel}: lacks [lints] workspace = true"]


def check_workspace_lints(rel, text):
    clippy = tomllib.loads(text).get("workspace", {}).get("lints", {}).get("clippy", {})
    return [f"{rel}: workspace clippy lint {n} is not deny" for n in REQUIRED_DENY
            if clippy.get(n) != "deny"]


def check_code(rel, text):
    problems = []
    for n, line in code_lines(text):
        if PANICKY.search(line):
            problems.append(f"{rel}:{n}: panicking call outside tests: {line.strip()}")
        if rel.startswith("crates/") and PRINT.search(line):
            problems.append(f"{rel}:{n}: print macro outside tests (use let _ = writeln!): {line.strip()}")
        if not is_mac(rel) and re.search(r"\bunsafe\b", line):
            problems.append(f"{rel}:{n}: unsafe outside crates/mac")
    for m in ATTR.finditer(text):
        names = {p.strip().split("::")[-1] for p in m.group(2).split(",") if "=" not in p}
        if not names & DENIED:
            continue
        sources_ok = (rel.startswith(("crates/sources/", "spikes/")) and m.group(1) == "allow"
                      and names == {"disallowed_methods"} and "reason" in m.group(2))
        mac_ok = (is_mac(rel) and m.group(1) == "allow"
                  and names == {"unsafe_code"} and "reason" in m.group(2))
        if not (sources_ok or mac_ok):
            n = text.count("\n", 0, m.start()) + 1
            problems.append(f"{rel}:{n}: {m.group(1)} of a denied lint: {' '.join(m.group(0).split())}")
    return problems


def check_safety(rel, text):
    if not is_mac(rel):
        return []
    lines, problems = text.splitlines(), []
    for i, line in enumerate(lines):
        if line.lstrip().startswith("//") or not UNSAFE_BLOCK.search(line):
            continue
        j = i - 1
        while j >= 0 and lines[j].lstrip().startswith("//") and "SAFETY:" not in lines[j]:
            j -= 1
        if j < 0 or "// SAFETY:" not in lines[j]:
            problems.append(f"{rel}:{i + 1}: unsafe block without a // SAFETY: comment above it")
    return problems


def check_glob(rel, text):
    return [f"{rel}:{text.count(chr(10), 0, m.start()) + 1}: glob re-export"
            for m in GLOB.finditer(text)]


def check_comments(rel, text):
    problems, run, start = [], 0, 0
    lines = text.splitlines() + [""]
    for n, line in enumerate(lines, 1):
        if line.lstrip().startswith("//"):
            if run == 0:
                start = n
            run += 1
            continue
        header = start == 1 and re.search(r"licen[cs]e|SPDX|copyright", "\n".join(lines[:run]), re.I)
        if run > 2 and not header:
            problems.append(f"{rel}:{start}: {run} consecutive comment lines (limit 2)")
        run = 0
    return problems


def scan(root):
    problems = []
    for manifest in sorted(list(root.glob("crates/*/Cargo.toml")) + list(root.glob("spikes/*/Cargo.toml"))):
        crate = manifest.parent
        problems += check_manifest(str(manifest.relative_to(root)), manifest.read_text())
        roots = [p for p in (crate / "src/lib.rs", crate / "src/main.rs") if p.exists()]
        if not roots:
            problems.append(f"{crate.relative_to(root)}: no src/lib.rs or src/main.rs")
        for r in roots:
            problems += check_root(str(r.relative_to(root)), r.read_text())
    problems += check_workspace_lints("Cargo.toml", (root / "Cargo.toml").read_text())
    files = sorted(list(root.glob("crates/*/**/*.rs")) + list(root.glob("spikes/*/**/*.rs")))
    test_files = set()
    for f in files:
        _, mods = strip_tests(f.read_text())
        for m in mods:
            base = f.parent if f.name in ("lib.rs", "main.rs", "mod.rs") else f.with_suffix("")
            test_files |= {base / f"{m}.rs", base / m / "mod.rs"}
    for f in files:
        rel, text = str(f.relative_to(root)), f.read_text()
        in_tests = "tests" in f.relative_to(root).parts[2:3] or f in test_files
        if not in_tests:
            problems += check_code(rel, strip_tests(text)[0])
        problems += check_safety(rel, text) + check_glob(rel, text) + check_comments(rel, text)
    return problems


BAD = {
    "root lacks forbid": (check_root, "crates/x/src/lib.rs", "#![deny(clippy::string_slice)]\n"),
    "root lacks string_slice": (check_root, "crates/x/src/lib.rs", "#![forbid(unsafe_code)]\n"),
    "mac root uses forbid": (check_root, "crates/mac/src/lib.rs",
                             "#![forbid(unsafe_code)]\n#![deny(clippy::string_slice)]\n"),
    "mac root lets an unsafe block go undocumented": (
        check_root, "crates/mac/src/lib.rs", "#![deny(unsafe_code)]\n#![deny(clippy::string_slice)]\n"
        "#![deny(clippy::multiple_unsafe_ops_per_block)]\n"),
    "manifest without lints": (check_manifest, "crates/x/Cargo.toml", "[package]\nname = \"x\"\n"),
    "workspace lint warn": (check_workspace_lints, "Cargo.toml",
                            "[workspace.lints.clippy]\n" + "".join(f'{n} = "deny"\n' for n in REQUIRED_DENY)
                            .replace('panic = "deny"', 'panic = "warn"')),
    "unwrap": (check_code, "crates/x/src/a.rs", "fn f() { g().unwrap(); }\n"),
    "expect": (check_code, "crates/x/src/a.rs", "fn f() { g().expect(\"x\"); }\n"),
    "panic": (check_code, "crates/x/src/a.rs", "fn f() { panic!(\"x\"); }\n"),
    "todo": (check_code, "crates/x/src/a.rs", "fn f() { todo!() }\n"),
    "unimplemented": (check_code, "crates/x/src/a.rs", "fn f() { unimplemented!() }\n"),
    "unreachable": (check_code, "crates/x/src/a.rs", "fn f() { unreachable!() }\n"),
    "allow denied lint": (check_code, "crates/x/src/a.rs", "#[allow(clippy::unwrap_used)]\nfn f() {}\n"),
    "expect denied lint": (check_code, "crates/x/src/a.rs",
                           "#[expect(clippy::panic, reason = \"x\")]\nfn f() {}\n"),
    "spawn allow outside sources": (check_code, "crates/core/src/a.rs",
                                    "#[allow(clippy::disallowed_methods, reason = \"x\")]\nfn f() {}\n"),
    "spawn allow without reason": (check_code, "crates/sources/src/a.rs",
                                   "#[allow(clippy::disallowed_methods)]\nfn f() {}\n"),
    "unsafe outside mac": (check_code, "crates/x/src/a.rs", "fn f() { unsafe { g() } }\n"),
    "unsafe allow outside mac": (check_code, "crates/x/src/a.rs",
                                 "#[allow(unsafe_code, reason = \"x\")]\nfn f() {}\n"),
    "mac unsafe allow without reason": (check_code, "crates/mac/src/a.rs", "#[allow(unsafe_code)]\nfn f() {}\n"),
    "unsafe without SAFETY": (check_safety, "crates/mac/src/a.rs", "fn f() {\n    unsafe { g() }\n}\n"),
    "unsafe impl without SAFETY": (check_safety, "crates/mac/src/a.rs", "unsafe impl Send for X {}\n"),
    "eprintln": (check_code, "crates/x/src/a.rs", "fn f() { eprintln!(\"x\"); }\n"),
    "println": (check_code, "crates/x/src/a.rs", "fn f() { println!(\"x\"); }\n"),
    "glob re-export": (check_glob, "crates/x/src/lib.rs", "pub use crate::a::*;\n"),
    "long comment": (check_comments, "crates/x/src/a.rs", "fn f() {}\n// a\n// b\n// c\nfn g() {}\n"),
    "unwrap in a non-test module after a test module": (
        lambda rel, text: check_code(rel, strip_tests(text)[0]), "crates/x/src/a.rs",
        "#[cfg(test)]\nmod tests {\n    fn t() { g().unwrap(); }\n}\nfn f() { g().unwrap(); }\n"),
}

GOOD = {
    "clean root": (check_root, "crates/x/src/lib.rs", "#![forbid(unsafe_code)]\n#![deny(clippy::string_slice)]\n"),
    "clean mac root": (check_root, "crates/mac/src/lib.rs", "#![deny(unsafe_code)]\n#![deny(clippy::string_slice)]\n"
                       "#![deny(\n    clippy::undocumented_unsafe_blocks,\n    clippy::multiple_unsafe_ops_per_block\n)]\n"),
    "unwrap in a test module": (lambda rel, text: check_code(rel, strip_tests(text)[0]), "crates/x/src/a.rs",
                                "#[cfg(test)]\nmod tests {\n    fn t() { g().unwrap(); }\n}\n"),
    "spawn allow in sources": (check_code, "crates/sources/src/a.rs",
                               "#[allow(clippy::disallowed_methods, reason = \"the spawn\")]\nfn f() {}\n"),
    "mac scoped unsafe allow": (check_code, "crates/mac/src/a.rs",
                                "#[allow(unsafe_code, reason = \"the one FFI call\")]\nfn f() {}\n"),
    "unsafe with SAFETY": (check_safety, "crates/mac/src/a.rs", "// SAFETY: g has no preconditions.\nunsafe { g() }\n"),
    "writeln to stderr": (check_code, "crates/x/src/a.rs",
                          "fn f() { let _ = writeln!(std::io::stderr(), \"x\"); }\n"),
    "licence header": (check_comments, "crates/x/src/a.rs", "// Copyright x\n// Licensed MIT\n// see LICENSE\nfn f() {}\n"),
    "two comment lines": (check_comments, "crates/x/src/a.rs", "// a\n// b\nfn f() {}\n"),
}


def selfcheck():
    failed = [f"did not fire: {name}" for name, (check, rel, text) in BAD.items() if not check(rel, text)]
    failed += [f"fired on a clean sample: {name}: {check(rel, text)}"
               for name, (check, rel, text) in GOOD.items() if check(rel, text)]
    for line in failed:
        print(f"har selfcheck: {line}")
    print(f"har selfcheck: {len(BAD)} bad samples, {len(GOOD)} clean samples, {len(failed)} wrong")
    return 1 if failed else 0


def main():
    if sys.argv[1:] == ["--selfcheck"]:
        return selfcheck()
    problems = scan(ROOT)
    for p in problems:
        print(f"har: {p}")
    print(f"har: {len(problems)} problems")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
