#!/usr/bin/env python3
"""Per-crate src lines may not grow past the fork point's without `raise: crate <name> +N` in a
change file this branch adds (D43 and its successors).
Incident: yi-tui's 10,000-line ceiling was prose only, so the crate reached 10,913
before anyone measured it."""
import sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import SRC, fail, fork, no_fork, texts
from check_changes import raised


def sizes(files):
    out = {}
    for path, text in files.items():
        crate = path.split("/")[1]
        out[crate] = out.get(crate, 0) + len(text.splitlines())
    return out


base = fork()
if base is None:
    no_fork("crate_size")
now, was = sizes(texts(SRC)), sizes(texts(SRC, base))
limit = {n: was.get(n, 0) + raised(base, f"crate {n}") for n in now}
errs = [f"crates/{n}/src {s} lines > {limit[n]} (fork {was.get(n, 0)}); a change file here says `raise: crate {n} +{s - was.get(n, 0)}`"
        for n, s in sorted(now.items()) if s > limit[n]]
fail(errs, "crate_size (" + ", ".join(f"{n} {s}/{limit[n]}" for n, s in sorted(now.items())) + ")")
