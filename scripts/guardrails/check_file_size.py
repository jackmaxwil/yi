#!/usr/bin/env python3
"""File ceiling 1,200 production lines, src/ only (D31: a surveyed core/ is 67% test LOC — an unscoped
ratchet fires forever). Inline #[cfg(test)] items do not count: counted whole, dispatch.rs sat at the
cap with 845 test lines, and 36 files packed to within 100 lines of it instead of splitting. Past
1,000 production lines a file may not grow on a branch, so the next addition splits it at a seam."""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import SRC, fail, fork, no_fork, test_mask, texts

HARD, SOFT = 1200, 1000


def production(text, off=""):
    return len(text.splitlines()) if off == "whole" else test_mask(text).count(False)


def errors(now, was, off=""):
    """`now` and `was` map a src path to its text on the branch and at the fork."""
    errs = []
    for path, text in sorted(now.items()):
        lines, before = production(text, off), (0 if off == "fork" else production(was.get(path, ""), off))
        if lines > HARD and off != "hard":
            errs.append(f"{path}: {lines} production lines > {HARD}")
        elif lines > SOFT and lines > before and off != "soft":
            errs.append(f"{path}: {lines} production lines, {before} at the fork; a file past {SOFT} may not grow, split it at a seam first")
    return errs


def selfcheck(off=""):
    """A file shaped like dispatch.rs, 600 production and 800 test lines, passes and one of 1,201
    fails; growth fails past 1,000 (1,000 to 1,001, 1,150 to 1,151) and passes at it (999 to 1,000,
    900 to 901, and 1,100 left as it was)."""
    prod = lambda n: "\n".join(f"fn f{i}() {{}}" for i in range(n))
    tests = "\n".join(["#[cfg(test)]", "mod tests {", *[f"    fn t{i}() {{}}" for i in range(798)], "}"])
    was = {"crates/x/src/" + name: prod(n) for name, n in
           (("edge.rs", 1000), ("big.rs", 1150), ("under.rs", 999), ("small.rs", 900), ("still.rs", 1100))}
    now = {"crates/x/src/dispatch.rs": prod(600) + "\n" + tests, "crates/x/src/over.rs": prod(1201),
           **{path: text + "\nfn g() {}" for path, text in was.items() if "still" not in path},
           "crates/x/src/still.rs": was["crates/x/src/still.rs"]}
    want = ["crates/x/src/big.rs: 1151 production lines, 1150 at the fork; a file past 1000 may not grow, split it at a seam first",
            "crates/x/src/edge.rs: 1001 production lines, 1000 at the fork; a file past 1000 may not grow, split it at a seam first",
            "crates/x/src/over.rs: 1201 production lines > 1200"]
    found = errors(now, was, off)
    return [] if found == want else [f"gate said {found}, not {want}"]


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("whole", "hard", "soft", "fork"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
        fail(errs, "file_size selfcheck")
        sys.exit(0)
    base = fork()
    if base is None:
        no_fork("file_size")
    fail(errors(texts(SRC), texts(SRC, base)), "file_size")
