#!/usr/bin/env python3
"""Function ceiling 150 lines, src/ only (promised in check_file_size.py's docstring; brace
counting stands in for syn — good enough for a ceiling, revisit if it misfires)."""
import pathlib, re, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, code_lines, normal_code, src_files, prod_lines, fail

CEILING = 150
FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?(?:const\s+)?(?:unsafe\s+)?fn\s+(\w+)")

def fn_sizes(lines, off=""):
    sizes = []
    lines = normal_code("\n".join(lines), off).splitlines()
    index = 0
    while index < len(lines):
        match = FN.match(lines[index])
        if not match or "{" not in "".join(lines[index:index + 3]):
            index += 1
            continue
        name, start, depth, opened = match.group(1), index, 0, False
        while index < len(lines):
            depth += lines[index].count("{") - lines[index].count("}")
            if lines[index].count("{"):
                opened = True
            index += 1
            if opened and depth <= 0:
                break
        sizes.append((name, start + 1, index - start))
    return sizes

def selfcheck(off=""):
    """A char or byte literal brace once left a fn unclosed, measuring it to the file's end; a fn
    below a mid-file test helper, shaped like provider.rs's, is production and is measured."""
    lines = ["fn a<'x>(s: &'x str) -> bool {", "    s.contains(['{', '}']) || b'{' == 0", "}", "fn b() {}"]
    sizes = [size for _, _, size in fn_sizes(lines, off)]
    errs = [] if sizes == [3, 1] else [f"sizes {sizes}, not [3, 1]: a '{{' or b'{{' literal or a lifetime moved a fn's end"]
    text = "\n".join(["impl Registry {", "    #[cfg(test)]",
                      "    pub(crate) fn settle_wired_for_test(&self) -> Option<Settle> {",
                      "        self.settle.lock().ok().and_then(|slot| slot.clone())", "    }", "",
                      "    pub fn with_auth(self) -> Self {", *["        self.step();"] * CEILING, "    }", "}"])
    seen = [(name, size) for name, _, size in fn_sizes(code_lines(text, off))]
    if seen != [("with_auth", CEILING + 2)]:
        errs.append(f"below a mid-file test helper the scan saw {seen}, not [('with_auth', {CEILING + 2})]")
    for shape, expected in (
        ('const EXAMPLE: &str = r#"\n#[cfg(test)]\n"#;\nfn production() {}\n', [("production", 4, 1)]),
        ('/*\n#[cfg(test)]\n*/\nfn production() {}\n', [("production", 4, 1)]),
        ('fn production() {\n    let text = r#"\n{\n"#;\n}\nfn after() {}\n',
         [("production", 1, 5), ("after", 6, 1)]),
        ('fn production() {\n    /*\n    {\n    */\n}\nfn after() {}\n',
         [("production", 1, 5), ("after", 6, 1)]),
    ):
        seen = fn_sizes(code_lines(shape, off))
        if seen != expected:
            errs.append(f"string or comment changed production function sizes: {seen}, not {expected}")
    return errs

if "--selfcheck" in sys.argv:
    errs = selfcheck()
    for off in ("chars", "first", "markers", "literals"):
        if not selfcheck(off):
            errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
    fail(errs, "fn_size selfcheck")
    sys.exit(0)

errs = []
for f in src_files():
    for name, line, size in fn_sizes(prod_lines(f)):
        if size > CEILING:
            errs.append(f"{f.relative_to(ROOT)}:{line} fn {name}: {size} lines > {CEILING}")
fail(errs, "fn_size")
