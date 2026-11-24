#!/usr/bin/env python3
"""Checks one scene's element tree against what the mockup draws.

Usage: parity.py --tree TREE.json (--pane SLUG | --frame N) [--min 0.97] [--fixtures DIR]
       parity.py --selfcheck
TREE.json is what Rune's `--tree` writes. A whole frame is checked against frames/N: what intended.json lists for
`frame N` is removed, and so is each `string` or `contains` it lists for a pane that was read from that frame. Exit 0 when at least --min of the mockup's strings are present and
no size differs by more than 1 px and every size part is found, after removing what intended.json lists (a `string`, a `contains`, or a `size`: `part.dim`, or `part`
for a part with no element yet); 1 otherwise; 2 on bad input.
"""
import argparse
import json
import re
import sys
from pathlib import Path

FIXTURES = Path(__file__).resolve().parents[2] / "fixtures/mockup"
TOLERANCE = 1.0
KEY_PATTERNS = {"row": re.compile(r"\.row\."), "code-line": re.compile(r"\.L\d+$")}
# A fixture read from one part of a pane measures that part's own lines, which are no list rows.
PANE_ROWS = {"findings-gate": re.compile(r"\.gate\.check\.\d+$"), "findings-place": re.compile(r"\.place\."),
             "findings-checks": re.compile(r"\.rule\.[^.]+$"), "settings-groups": re.compile(r"\.group\.")}


def squash(s):
    return re.sub(r"\s+", "", s)


def tree_texts(tree):
    """Every text, label and help the tree shows, in tree order; exiting or fully transparent elements are not shown."""
    out = []
    for e in tree.get("elements", []):
        if e.get("exiting") or e.get("opacity", 1) == 0:
            continue
        out += [squash(e[k]) for k in ("text", "label", "help") if e.get(k)]
    return out


def present(needle, texts, joined):
    n = squash(needle)
    return any(n in t for t in texts) or n in joined


def key_matches(part, key, pane=None):
    pattern = PANE_ROWS.get(pane) if part == "row" and pane in PANE_ROWS else KEY_PATTERNS.get(part)
    return bool(pattern.search(key)) if pattern else key == part or key.endswith("." + part)


def size_findings(tree, sizes, waived, pane=None):
    """Lines for sizes that are out, and the parts no key in the tree matches and intended.json does not waive."""
    out, unmeasured = [], []
    for s in sizes:
        if not s.get("check"):
            continue
        hits = [e for e in tree.get("elements", []) if e.get("key") and key_matches(s["part"], e["key"], pane) and len(e.get("rect", [])) == 4]
        if not hits:
            if s["part"] not in waived and not all(f"{s['part']}.{d}" in waived for d in s["check"]):
                unmeasured.append(s["part"])
            continue
        for dim in s["check"]:
            if f"{s['part']}.{dim}" in waived:
                continue
            idx = 3 if dim == "height" else 2
            best = min(hits, key=lambda e: abs(e["rect"][idx] - s[dim]))
            got = best["rect"][idx]
            if abs(got - s[dim]) > TOLERANCE:
                out.append(f"size {s['part']}.{dim}: mockup {s[dim]}, scene {round(got, 2)} ({best['key']})")
    return out, unmeasured


def frame_intended(intended, frame, panes):
    """What a whole frame may leave out: its own entries, and the words its panes may leave out (never their sizes)."""
    carried = [{**i, "pane": frame} for i in intended if i.get("pane") in panes and "size" not in i]
    return [i for i in intended if i.get("pane") in (frame, "*")] + carried


def check(tree, text_doc, sizes_doc, intended, pane, minimum):
    own = [i for i in intended if i.get("pane") in (pane, "*")]
    skip_strings = {squash(i["string"]) for i in own if "string" in i}
    skip_parts = [squash(i["contains"]) for i in own if "contains" in i]
    waived = {i["size"] for i in own if "size" in i}
    expected = [s for s in text_doc["strings"] if squash(s) not in skip_strings and not any(c in squash(s) for c in skip_parts)]
    texts = tree_texts(tree)
    joined = "".join(texts)
    missing = [s for s in expected if not present(s, texts, joined)]
    ratio = 1.0 if not expected else (len(expected) - len(missing)) / len(expected)
    out, unmeasured = size_findings(tree, sizes_doc["sizes"], waived, pane)
    return {"ratio": ratio, "expected": len(expected), "missing": missing, "sizes_out": out, "unmeasured": unmeasured,
            "ok": ratio >= minimum and not out and not unmeasured}


def report(r, minimum):
    lines = [f"missing: {s}" for s in r["missing"]] + r["sizes_out"] + [f"unmeasured: {p}" for p in r["unmeasured"]]
    lines.append(f"strings {r['expected'] - len(r['missing'])}/{r['expected']} = {r['ratio']:.3f} (need {minimum}); sizes out {len(r['sizes_out'])}; unmeasured {len(r['unmeasured'])}")
    return "\n".join(lines)


def load(path):
    try:
        return json.loads(Path(path).read_text())
    except (OSError, ValueError) as e:
        sys.exit(f"parity: cannot read {path}: {e}")


def selfcheck():
    text = {"strings": ["Agents", "Doing now", "Rate-limit the upload endpoint", "waiting on a child", "$0.38"]}
    sizes = {"sizes": [{"part": "row", "check": ["height"], "height": 20}, {"part": "footer", "check": ["height"], "height": 20}]}

    def tree(strings, row_h, with_footer=True):
        els = [{"key": f"agents-1.row.{i}", "text": s, "rect": [0, 0, 100, row_h], "opacity": 1} for i, s in enumerate(strings)]
        if with_footer:
            els.append({"key": "agents-1.footer", "rect": [0, 0, 100, 20]})
        return {"elements": els}

    good = check(tree(text["strings"], 20), text, sizes, [], "agents", 0.97)
    assert good["ok"], ("a scene that draws everything at the right size must pass", good)
    split = check({"elements": [{"key": "agents-1.row.0", "text": "Rate-limit the", "rect": [0, 0, 1, 20]}, {"key": "agents-1.row.1", "text": " upload endpoint", "rect": [0, 0, 1, 20]}]},
                  {"strings": ["Rate-limit the upload endpoint"]}, {"sizes": []}, [], "agents", 1.0)
    assert split["ok"], ("text split across consecutive elements still counts", split)
    short = check(tree(text["strings"][:2], 20), text, sizes, [], "agents", 0.97)
    assert not short["ok"] and len(short["missing"]) == 3, ("a scene missing three strings must fail and name them", short)
    tall = check(tree(text["strings"], 25), text, sizes, [], "agents", 0.97)
    assert not tall["ok"] and tall["sizes_out"], ("a row 5 px too tall must fail", tall)
    near = check(tree(text["strings"], 20.9), text, sizes, [], "agents", 0.97)
    assert near["ok"], ("0.9 px off is within the tolerance", near)
    waived = check(tree(text["strings"][:2], 25), text, sizes,
                   [{"pane": "agents", "string": s, "reason": "t"} for s in text["strings"][2:]] + [{"pane": "agents", "size": "row.height", "reason": "t"}], "agents", 0.97)
    assert waived["ok"], ("intended entries remove strings and sizes", waived)
    part = check(tree(["Agents", "Doing now"], 20), {"strings": ["Agents", "9 settings differ from the defaults"]}, sizes,
                 [{"pane": "agents", "contains": "9 settings differ", "reason": "t"}], "agents", 1.0)
    assert part["ok"], ("a contains entry removes every string holding it", part)
    other = check(tree(text["strings"][:2], 20), text, sizes, [{"pane": "logs", "string": x, "reason": "t"} for x in text["strings"][2:]], "agents", 0.97)
    assert not other["ok"], ("another pane's intended entry must not apply", other)
    gone = check(tree(text["strings"], 20, with_footer=False), text, sizes, [], "agents", 0.97)
    assert not gone["ok"] and gone["unmeasured"] == ["footer"], ("a part no key matches must fail", gone)
    for entry in ("footer", "footer.height"):
        free = check(tree(text["strings"], 20, with_footer=False), text, sizes, [{"pane": "agents", "size": entry, "reason": "t"}], "agents", 0.97)
        assert free["ok"] and not free["unmeasured"], (f"intended size {entry} waives an absent part", free)
    elsewhere = check(tree(text["strings"], 20, with_footer=False), text, sizes, [{"pane": "logs", "size": "footer", "reason": "t"}], "agents", 0.97)
    assert not elsewhere["ok"], ("another pane's waiver must not apply", elsewhere)
    whole = frame_intended([{"pane": "agents", "string": "$0.38", "reason": "t"}, {"pane": "agents", "size": "row.height", "reason": "t"},
                            {"pane": "logs", "string": "Agents", "reason": "t"}, {"pane": "frame 6", "string": "Doing now", "reason": "t"}], "frame 6", {"agents"})
    assert [i.get("string") for i in whole] == ["Doing now", "$0.38"], ("a frame takes its own entries and its panes' words, not their sizes or another pane's", whole)
    assert key_matches("row", "findings-0.gate.check.1", "findings-gate") and not key_matches("row", "findings-0.row.x", "findings-gate"), "a part's fixture measures its own lines"
    for part, key, hit in (("pane-header", "agents-1.pane-header", True), ("table-header", "agents-1.table-header", True), ("footer", "agents-1.footer", True),
                           ("pane-header", "agents-1.head", False), ("footer", "agents-1.foot", False), ("table-header", "agents-1.pane-header", False)):
        assert key_matches(part, key) == hit, ("size parts match the keys <pane>.pane-header, .table-header and .footer", part, key)
    print("parity selfcheck ok")


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--tree")
    ap.add_argument("--pane")
    ap.add_argument("--frame")
    ap.add_argument("--min", type=float, default=0.97)
    ap.add_argument("--fixtures", type=Path, default=FIXTURES)
    ap.add_argument("--selfcheck", action="store_true")
    a = ap.parse_args()
    if a.selfcheck:
        selfcheck()
        return 0
    if not a.tree or bool(a.pane) == bool(a.frame):
        ap.error("give --tree and exactly one of --pane or --frame")
    folder, name = ("panes", a.pane) if a.pane else ("frames", a.frame)
    stem = a.fixtures / folder / name
    text = load(f"{stem}.text.json" if a.pane else f"{stem}.strings.json")
    sizes = load(f"{stem}.sizes.json")
    intended = load(a.fixtures / "intended.json")
    if a.frame:
        panes = {p.name[:-len(".text.json")] for p in (a.fixtures / "panes").glob("*.text.json") if str(load(p).get("frame")) == name}
        intended = frame_intended(intended, f"frame {name}", panes)
    r = check(load(a.tree), text, sizes, intended, name if a.pane else f"frame {name}", a.min)
    print(report(r, a.min))
    return 0 if r["ok"] else 1


if __name__ == "__main__":
    sys.exit(main())
