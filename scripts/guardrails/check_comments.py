#!/usr/bin/env python3
"""Comment length cap (2 lines, §19) plus a shrink-only volume ratchet outside yi-types.
Incident: the rule tested the sigil and nothing enforced it, so 82 blocks reached 4+ lines and
1,017 doc-comment lines accumulated in crates the grant never covered (D49). Volume and the
over-cap count may not pass the fork point's without `raise: comments +N` / `raise: over-cap +N`
in a change file this branch adds.

Also rejects pointer-only comments, in every crate: strip every row/decision id and the fact
must still stand on its own. A reader without the design doc open gets nothing from "V12:
verbatim, append-only"; ids are trailing pointers, never the payload. Only the volume ratchet
skips yi-types, where a schema fact earns its line."""
import re, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import SRC, code_lines, fail, fork, no_fork, texts
from check_changes import raise_errors, raised

CAP = 2
LICENSE = re.compile(r"SPDX|Copyright|\bMIT\b|Apache-2\.0|BSD|licen[sc]e", re.I)
GRANTS = {"Incident", "Invariant"}
TAG = re.compile(r"^//[/!]?\s*([A-Z][a-z]+):")
SIGIL = re.compile(r"^(///|//!|//|/\*+|\*/|\*)\s*")
CODE = re.compile(r"`[^`]*`|\[[^\]]*\]")
ID = re.compile(r"\b[A-Z]\d{1,3}\b")
# Calibrated over the 187 id-bearing comments in the tree at 0.79.0: 8 words separates the 10
# that said nothing without the design doc from the shortest comments that stood alone.
MIN_WORDS = 8

def pointer_only(body):
    """A code span counts as one word — masking it outright called `models.autoReview` empty."""
    txt = " ".join(SIGIL.sub("", line) for line in body)
    if not ID.search(CODE.sub(" ", txt)):
        return False
    rest = ID.sub(" ", CODE.sub(" span ", txt))
    return len([w for w in re.split(r"[\s.,;:()§—-]+", rest) if w]) < MIN_WORDS

def comment_runs(text):
    runs, cur, in_block = [], None, False
    for i, line in enumerate(code_lines(text), 1):
        s = line.strip()
        if in_block:
            is_comment = True
            if "*/" in s:
                in_block = False
        elif s.startswith("/*"):
            is_comment = True
            in_block = "*/" not in s
        else:
            is_comment = s.startswith("//")
        if is_comment:
            if cur and cur[1] == i - 1:
                cur[1], cur[2] = i, cur[2] + [s]
            else:
                if cur:
                    runs.append(cur)
                cur = [i, i, [s]]
        elif cur:
            runs.append(cur)
            cur = None
    if cur:
        runs.append(cur)
    return runs

def scan(files):
    over, volume, tags, pointers = [], 0, [], []
    for rel, text in files.items():
        for start, end, body in comment_runs(text):
            n = end - start + 1
            if rel.split("/")[1] != "types":
                volume += n
            if pointer_only(body):
                pointers.append(f"{rel}:{start}: pointer-only comment; state the fact, keep the id a trailing pointer")
            if n > CAP and not (start == 1 and any(LICENSE.search(l) for l in body)):
                over.append(f"{rel}:{start}: comment run of {n} lines > {CAP}")
            m = TAG.match(body[0])
            if m and m.group(1) not in GRANTS:
                tags.append(f"{rel}:{start}: '{m.group(1)}:' is not a §19 grant ({'|'.join(sorted(GRANTS))})")
    return over, volume, tags, pointers


base = fork()
if base is None:
    no_fork("comments")
over, volume, tags, pointers = scan(texts(SRC))
was_over, was_volume, _, _ = scan(texts(SRC, base))
cap_raise, volume_raise = raised(base, "over-cap"), raised(base, "comments")
cap_errs = raise_errors("over-cap", len(over), len(was_over), cap_raise)
errs = tags + pointers + (over if len(over) > len(was_over) + cap_raise else []) + cap_errs
errs += raise_errors("comments", volume, was_volume, volume_raise)
fail(errs, f"comments ({volume}/{was_volume + volume_raise} lines outside yi-types, {len(over)}/{len(was_over) + cap_raise} over cap)")
