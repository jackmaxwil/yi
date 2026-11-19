#!/usr/bin/env python3
"""Pulls every sample table, the text and the sizes out of the built mockup page into fixtures/mockup.

Usage: extract.py [--page PAGE] [--out DIR] [--selfcheck]
Needs headless Chrome. Running it twice gives byte-identical files; --selfcheck proves that and the row counts.
"""
import argparse
import json
import re
import sys
import tempfile
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import chrome  # noqa: E402
import readme  # noqa: E402
import rows as rowmod  # noqa: E402
import spec  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
PAGE = Path.home() / "Development/t3-rune-port/mockups/yi-desktop.html"
SEPARATORS = re.compile(r"[\s·/|,.:;]*")
TOKEN_NAMES = {
    "--accent": "accent", "--need": "needs_you", "--ok": "ok", "--bad": "bad", "--fg": "text.1", "--dim": "text.2", "--faint": "text.3",
    "--kw": "code.keyword", "--ty": "code.type", "--lit": "code.string", "--nu": "code.number", "--cmt": "code.comment",
    "--sans": "font.text", "--pa": "surface.pane", "--pf": "surface.pane_active", "--ga": "surface.window",
    "--row": "space.row", "--ln": "space.line", "--gut": "space.gutter",
}


def write(out, rel, obj):
    path = out / rel
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(obj, indent=1, ensure_ascii=False) + "\n")


def keep(strings):
    return [s for s in strings if not SEPARATORS.fullmatch(s)]


def frame_spec(frame):
    panes = [p for p in spec.PANES if p[1] == frame]
    roots = []
    for slug, _, sel, row, _ in panes:
        row_defs = [[n, row if s_ is None else s_, c, after] for n, s_, c, after in spec.ROW_DEFS if row or n != "row"]
        if slug in spec.WHOLE:
            defs = [list(d) for d in spec.WHOLE[slug]] + [d for d in row_defs if d[0] == "row"]
        else:
            defs = [list(d) for d in spec.PANE_DEFS] + row_defs + [list(d) for d in spec.EXTRA_DEFS.get(slug, [])]
        roots.append({"slug": slug, "sel": sel, "defs": defs})
    generic = ".agr:not(.hd),.spn.xr,.lgl,.fd.xr,.ast,.ctr,.vmr,.dmn,.plg,.dgn,.stg,.vrr,.f2,.fx,.thm,.tk"
    frame_defs = [list(d) for d in spec.WHOLE["shell"] + spec.WHOLE["shell-folded"][2:] + spec.PANE_DEFS] + [["row", generic, ["height"], 0], ["row-detail", ".xd, .lgx", [], 1]]
    return {
        "roots": roots, "frame_defs": frame_defs, "scrapes": spec.SCRAPES.get(frame, {}),
        "data": spec.DATA if frame == 1 else [],
    }, " ".join(sorted({p[4] for p in panes if p[4]}))


def run_all(page_text):
    def one(frame):
        s, query = frame_spec(frame)
        return frame, chrome.run_frame(page_text, frame, s, query)

    with ThreadPoolExecutor(4) as pool:
        return dict(pool.map(one, spec.FRAMES))


def icons(data):
    helper = re.match(r'<svg class="i" width="14" height="14" viewBox="([^"]+)" fill="([^"]+)" stroke="([^"]+)" stroke-width="([^"]+)" stroke-linecap="([^"]+)" stroke-linejoin="([^"]+)">', data["I('x',14)"])
    if not helper:
        raise chrome.ChromeError("icon helper markup changed")
    vb, fill, stroke, width, cap, join = helper.groups()
    return {"helper": {"view_box": vb, "fill": fill, "stroke": stroke, "stroke_width": float(width), "line_cap": cap, "line_join": join},
            "icons": data["IC"]}


def themes(data, tokens):
    base = tokens["vars"]
    shaped = {}
    for key, th in data["THEMES"].items():
        values = {**base, **th["v"]}
        out = {}
        for var, name in TOKEN_NAMES.items():
            v = str(values.get(var, "")).strip()
            if not v:
                continue
            out[name] = float(v.removesuffix("px")) if re.fullmatch(r"-?\d*\.?\d+(px)?", v) else v
        shaped[key] = {"id": key, "name": th["n"], "note": th["d"], "dark": True, "values": out}
    return shaped


def generate(page_text, out):
    runs = run_all(page_text)
    raw = {"data": runs[1]["data"], "scrapes": {}}
    for frame in spec.FRAMES:
        raw["scrapes"].update(runs[frame]["scrapes"])
    for name in spec.DATA:
        if raw["data"].get(name) is None:
            raise chrome.ChromeError(f"the page defines no {name}")

    for name in spec.DATA:
        if name not in ("IC", "I('x',14)", "AM", "BL", "GR", "STATES"):
            write(out, f"data/{re.sub(r'[^A-Za-z0-9_]', '', name.split('.')[0])}.json", raw["data"][name])
    write(out, "data/icons.json", icons(raw["data"]))
    write(out, "data/palette.json", {k: raw["data"][k] for k in ("AM", "BL", "GR", "STATES")})
    for key, value in raw["scrapes"].items():
        if not key.startswith("detail:"):
            write(out, f"data/{key}.json", value)

    for th in themes(raw["data"], raw["scrapes"]["tokens"]).values():
        write(out, f"themes/{th['id']}.json", th)

    for frame in spec.FRAMES:
        f = runs[frame]["frame"]
        write(out, f"frames/{frame}.strings.json", {"frame": frame, "strings": keep(f["strings"]), "titles": f["titles"]})
        write(out, f"frames/{frame}.sizes.json", {"frame": frame, "sizes": f["sizes"]})
    pane_frames = {}
    for slug, frame, *_ in spec.PANES:
        p = runs[frame]["panes"][slug]
        if not p["found"]:
            raise chrome.ChromeError(f"pane {slug}: nothing found in frame {frame}")
        pane_frames[slug] = frame
        write(out, f"panes/{slug}.text.json", {"pane": slug, "frame": frame, "strings": keep(p["strings"]), "titles": p["titles"]})
        write(out, f"panes/{slug}.sizes.json", {"pane": slug, "frame": frame, "sizes": p["sizes"]})

    built = rowmod.build(raw)
    meta = {}
    for slug, r in built.items():
        write(out, f"{slug}.rows.json", r.rows)
        if r.meta():
            meta[slug] = r.meta()
    write(out, "meta.json", meta)
    (out / "README.md").write_text(readme.render(built, pane_frames, spec.PANES, spec.FRAMES))
    return built, raw


def files(root):
    return {p.relative_to(root).as_posix(): p.read_bytes() for p in sorted(root.rglob("*")) if p.is_file()}


def selfcheck(page_text):
    with tempfile.TemporaryDirectory() as a, tempfile.TemporaryDirectory() as b:
        built, raw = generate(page_text, Path(a))
        generate(page_text, Path(b))
        fa, fb = files(Path(a)), files(Path(b))
        assert fa == fb, [k for k in fa if fa[k] != fb.get(k)] or "file sets differ"
        counts = {"agents": "AG", "trace": "SP", "logs": "LG", "findings-table": "FD", "vms": "VMS", "daemons": "DMS", "lsp": "LSV",
                  "problems": "DG", "plugins": "PL", "debugger-variables": "DV", "debugger-watch": "DW", "debugger-breakpoints": "DB"}
        for slug, table in counts.items():
            assert len(built[slug].rows) == len(raw["data"][table]), (slug, len(built[slug].rows), len(raw["data"][table]))
        assert len(built["assets"].rows) == sum(1 for x in raw["data"]["AS"] if len(x) > 2)
        assert len(built["containers"].rows) == sum(1 for x in raw["data"]["CTS"] if len(x) > 2)
        for slug, r in built.items():
            ids = [x["id"] for x in r.rows]
            assert len(ids) == len(set(ids)), f"{slug}: duplicate row ids"
            assert all(x.get("parent") in ids or "parent" not in x for x in r.rows), f"{slug}: dangling parent"
            for group, note in r.notes.items():
                first = next((x for x in r.rows if x.get("group") == group), None)
                if note and first:
                    assert first["cells"].get("group.summary") == {"text": note}, f"{slug}: group {group!r} has no summary on its first row"
        assert sum(1 for r in built.values() for x in r.rows if "group.summary" in x["cells"]) >= 4, "grouped lists lost their summaries"
        print(f"selfcheck ok: {len(fa)} files, byte-identical on a second run")


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--page", type=Path, default=PAGE)
    ap.add_argument("--out", type=Path, default=ROOT / "fixtures/mockup")
    ap.add_argument("--selfcheck", action="store_true")
    a = ap.parse_args()
    text = a.page.read_text()
    if a.selfcheck:
        selfcheck(text)
        return
    generate(text, a.out)
    print(f"wrote {len(files(a.out))} files under {a.out}")


if __name__ == "__main__":
    main()
