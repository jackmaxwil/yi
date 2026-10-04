#!/usr/bin/env python3
"""Function ceiling 150 lines, src/ only (promised in check_file_size.py's docstring; brace
counting stands in for syn — good enough for a ceiling, revisit if it misfires)."""
import pathlib, re, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, src_files, prod_lines, fail

CEILING = 150
FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+(\w+)")

def fn_sizes(lines, off=""):
    sizes = []
    index = 0
    while index < len(lines):
        match = FN.match(lines[index])
        if not match or "{" not in "".join(lines[index:index + 3]):
            index += 1
            continue
        name, start, depth, opened = match.group(1), index, 0, False
        while index < len(lines):
            stripped = re.sub(r'"(?:[^"\\]|\\.)*"', '""', lines[index])
            if off != "chars":
                stripped = re.sub(r"'(?:[^'\\]|\\.)'", "''", stripped)
            stripped = re.sub(r"//.*", "", stripped)
            depth += stripped.count("{") - stripped.count("}")
            if stripped.count("{"):
                opened = True
            index += 1
            if opened and depth <= 0:
                break
        sizes.append((name, start + 1, index - start))
    return sizes

def selfcheck(off=""):
    """A char or byte literal brace once left a fn unclosed, measuring it to the file's end."""
    lines = ["fn a<'x>(s: &'x str) -> bool {", "    s.contains(['{', '}']) || b'{' == 0", "}", "fn b() {}"]
    sizes = [size for _, _, size in fn_sizes(lines, off)]
    return [] if sizes == [3, 1] else [f"sizes {sizes}, not [3, 1]: a '{{' or b'{{' literal or a lifetime moved a fn's end"]

if "--selfcheck" in sys.argv:
    errs = selfcheck()
    if not selfcheck("chars"):
        errs.append("selfcheck passes with char literals counted, so it refutes nothing")
    fail(errs, "fn_size selfcheck")
    sys.exit(0)

errs = []
for f in src_files():
    for name, line, size in fn_sizes(prod_lines(f)):
        if size > CEILING:
            errs.append(f"{f.relative_to(ROOT)}:{line} fn {name}: {size} lines > {CEILING}")
fail(errs, "fn_size")
