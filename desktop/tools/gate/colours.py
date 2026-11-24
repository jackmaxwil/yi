#!/usr/bin/env python3
"""Colour gate: a view draws colours a theme can reach, never a literal of its own.

Usage: colours.py [--selfcheck]. Exit 0 when clean, 1 with one line per literal.
"""

import re
import sys
from pathlib import Path

from har import strip_tests

ROOT = Path(__file__).resolve().parents[2]
SCANNED = ("crates/ui/src", "crates/panes/src", "crates/app/src")
LITERAL = re.compile(
    r"\b(?:rgba?|white|black|hex)\(\s*(?:0x|\d|\.\d|\"#)"
    r"|\bColor::(?:WHITE|BLACK)\b"
    r"|\bColor::(?:rgba?|srgb\w*|from_\w+|new)\("
)
# Exact files whose literals are not a view's colours, each with its reason.
ALLOWED = {
    "crates/ui/src/theme/scale.rs": "the theme defaults: every named value's Yi Dark colour",
    "crates/ui/src/theme/look.rs": "defines white and black, the constructors the defaults use",
    "crates/panes/src/web/pane/sample.rs": "a sample web page drawn in scenes, not the app's look",
    "crates/panes/src/report/pane/sample.rs": "a sample report page drawn in scenes, not the app's look",
    "crates/panes/src/remote/pane/sample.rs": "the remote desktop's own windows and words, not the app's look",
}


def check(rel, text):
    if rel in ALLOWED:
        return []
    code, _ = strip_tests(text)
    return [f"{rel}:{n}: colour literal in a view: {line.strip()}"
            for n, line in enumerate(code.splitlines(), 1)
            if not line.lstrip().startswith("//") and LITERAL.search(line)]


def in_tests(rel, path, test_files):
    parts = Path(rel).parts
    return "tests" in parts or path.name == "tests.rs" or path in test_files


def scan(root):
    files = sorted(f for d in SCANNED for f in (root / d).glob("**/*.rs"))
    test_files = set()
    for f in files:
        for m in strip_tests(f.read_text())[1]:
            base = f.parent if f.name in ("lib.rs", "main.rs", "mod.rs") else f.with_suffix("")
            test_files |= {base / f"{m}.rs", base / m / "mod.rs"}
    problems = []
    for f in files:
        rel = str(f.relative_to(root))
        if not in_tests(rel, f, test_files):
            problems += check(rel, f.read_text())
    missing = [rel for rel in ALLOWED if not (root / rel).exists()]
    return problems + [f"{rel}: allowed but missing; drop it from ALLOWED" for rel in missing]


BAD = {
    "rgb": "fn f() { colour(rgb(0x11, 0x12, 0x14)) }\n",
    "rgba": "fn f() { colour(rgba(14, 16, 20, 0.74)) }\n",
    "white": "fn f() { colour(white(0.043)) }\n",
    "black": "fn f() { colour(black(0.2)) }\n",
    "hex": "fn f() { rune::color::hex(\"#ffffff\") }\n",
    "Color::WHITE": "fn f() { Color::WHITE.alpha(0.3) }\n",
    "path-qualified": "fn f() { yd_core::tokens::rgb(24, 26, 33) }\n",
    "after a test module": "#[cfg(test)]\nmod tests {\n    fn t() {}\n}\nfn f() { white(0.1) }\n",
}

GOOD = {
    "a token": "fn f() { colour(tokens.fg) }\n",
    "a named value": "fn f() { colour(FILL_PRESS.of(tokens)) }\n",
    "a token at a strength": "fn f() { colour(white(tokens.lines)) }\n",
    "in a test module": "#[cfg(test)]\nmod tests {\n    fn t() { white(0.1); }\n}\n",
    "in a comment": "// white(0.1) was the old hover\nfn f() {}\n",
}


def selfcheck():
    failed = [f"did not fire: {name}" for name, text in BAD.items()
              if not check("crates/ui/src/x.rs", text)]
    failed += [f"fired on a clean sample: {name}" for name, text in GOOD.items()
               if check("crates/ui/src/x.rs", text)]
    if not check("crates/ui/src/theme/files.rs", BAD["rgb"]):
        failed.append("an unlisted file in the theme folder is allowed")
    if check("crates/ui/src/theme/scale.rs", BAD["rgb"]):
        failed.append("an allowed file fired")
    for line in failed:
        print(f"colours selfcheck: {line}")
    print(f"colours selfcheck: {len(BAD)} bad samples, {len(GOOD)} clean samples, {len(failed)} wrong")
    return 1 if failed else 0


def main():
    if sys.argv[1:] == ["--selfcheck"]:
        return selfcheck()
    problems = scan(ROOT)
    for p in problems:
        print(f"colours: {p}")
    print(f"colours: {len(problems)} literals")
    return 1 if problems else 0


if __name__ == "__main__":
    sys.exit(main())
