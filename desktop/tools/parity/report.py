#!/usr/bin/env python3
"""Writes the parity report and the pictures that go with it.

Usage: report.py [--scenes DIR] [--write docs/parity.md] [--pairs DIR]
       report.py --numbers [--scenes DIR]
Reads target/scenes (what `just scenes` rendered), fixtures/mockup and tools/parity/seen.json (what a person
sees differ, written by hand). --write puts the table between the report's two markers; --pairs saves, for every
frame, the scene's picture beside the mockup still as DIR/<frame>.png (sips converts, nothing else is needed).
"""
import argparse
import json
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))
import parity  # noqa: E402

ROOT = Path(__file__).resolve().parents[2]
STILLS = Path.home() / "Development/t3-rune-port/mockups"
# The still that shows each mockup frame, by the frame's number.
STILL = {1: "yd-L1", 4: "N1-notifications", 5: "M1-menu-bar", 6: "yd-P1", 7: "yd-P2", 8: "yd-P3", 9: "yd-P4", 10: "yd-F1", 11: "yd-S1",
         12: "yd-T1", 13: "yd-T7", 14: "yd-C1", 17: "yd-chat-hover", 19: "yd-CH0", 20: "x7-your-tree", 23: "yd-X5", 24: "yd-X6"}
START, END = "<!-- table: written by tools/parity/report.py -->", "<!-- end of table -->"
HIGH = 750


def rows(scenes):
    """One dict a pane fixture, then one for the whole first frame."""
    intended = parity.load(parity.FIXTURES / "intended.json")
    out = []
    for text_file in sorted((parity.FIXTURES / "panes").glob("*.text.json")):
        slug = text_file.name[: -len(".text.json")]
        text, sizes = parity.load(text_file), parity.load(text_file.with_name(f"{slug}.sizes.json"))
        out.append(score(scenes / slug / "mockup.tree.json", f"{slug}/mockup", slug, text, sizes, intended, slug, text["frame"]))
    frames = parity.FIXTURES / "frames"
    panes = {p.name[: -len(".text.json")] for p in (parity.FIXTURES / "panes").glob("*.text.json") if parity.load(p).get("frame") == 1}
    whole = parity.frame_intended(intended, "frame 1", panes)
    out.append(score(scenes / "shell/everything.tree.json", "shell/everything", "frame 1", parity.load(frames / "1.strings.json"),
                     parity.load(frames / "1.sizes.json"), whole, "whole-window", 1))
    return out


def score(tree, scene, pane, text, sizes, intended, name, frame):
    row = {"name": name, "frame": frame, "scene": scene if tree.is_file() else None, "picture": tree.with_name(tree.name.replace(".tree.json", ".png")),
           "waivers": [i for i in intended if i.get("pane") == pane]}
    if not tree.is_file():
        return row
    loaded = parity.load(tree)
    kept, bare = parity.check(loaded, text, sizes, intended, pane, 0.97), parity.check(loaded, text, sizes, [], pane, 0.97)
    checked = sum(len(s.get("check") or []) for s in sizes["sizes"])
    out = len(bare["sizes_out"]) + sum(len(s["check"]) for s in sizes["sizes"] if s["part"] in bare["unmeasured"])
    row.update(text=kept["ratio"], bare=bare["ratio"], strings=len(text["strings"]), missing=kept["missing"], sizes=(checked - out, checked), ok=kept["ok"])
    return row


def numbers(all_rows):
    have = [r for r in all_rows if r["scene"]]
    size = [r["sizes"][0] / r["sizes"][1] if r["sizes"][1] else 1.0 for r in have]
    intended = parity.load(parity.FIXTURES / "intended.json")
    return (f"frames {len(all_rows)}, with a scene {len(have)}, passing {sum(r['ok'] for r in have)}; "
            f"text mean {statistics.mean(r['text'] for r in have):.3f} worst {min(r['text'] for r in have):.3f} "
            f"(before exceptions mean {statistics.mean(r['bare'] for r in have):.3f} worst {min(r['bare'] for r in have):.3f}); "
            f"size mean {statistics.mean(size):.3f} worst {min(size):.3f}; exceptions {len(intended)}")


def table(all_rows, seen):
    lines = ["| Frame | Still | Scene | Text | Before exceptions | Sizes in | Exceptions | What a person sees differ |", "|---|---|---|---|---|---|---|---|"]
    for r in all_rows:
        still = STILL.get(r["frame"], "")
        waived = "; ".join(f"`{i.get('string') or i.get('contains') or i.get('size')}`: {i['reason']}" for i in r["waivers"]) or "none"
        differ = seen.get(r["name"], "")
        if not r["scene"]:
            lines.append(f"| {r['name']} ({r['frame']}) | {still} | none | | | | {waived} | {differ} |")
            continue
        missing = "".join(f" Not found: `{m}`." for m in r["missing"])
        lines.append(f"| {r['name']} ({r['frame']}) | {still} | `{r['scene']}` | {r['text']:.3f} | {r['bare']:.3f} | {r['sizes'][0]} of {r['sizes'][1]} | {waived} | {differ}{missing} |")
    return "\n".join(lines)


def bitmap(png, out, high):
    """`png` as a 24-bit bitmap `high` rows tall: (width, rows top to bottom, each width * 3 bytes)."""
    subprocess.run(["sips", "--resampleHeight", str(high), "-s", "format", "bmp", str(png), "--out", str(out)], check=True, capture_output=True)
    data = out.read_bytes()
    start, width, rows_, bits = int.from_bytes(data[10:14], "little"), int.from_bytes(data[18:22], "little"), int.from_bytes(data[22:26], "little", signed=True), int.from_bytes(data[28:30], "little")
    step, stride = bits // 8, (width * bits // 8 + 3) // 4 * 4
    lines = []
    for y in range(abs(rows_)):
        line = data[start + y * stride: start + y * stride + width * step]
        flat = bytearray(width * 3)
        for channel in range(3):
            flat[channel::3] = line[channel::step]
        lines.append(bytes(flat))
    return width, lines if rows_ < 0 else lines[::-1]


def pair(left, right, out):
    """Saves `left` beside `right`, both HIGH tall, as the picture `out`."""
    with tempfile.TemporaryDirectory() as tmp:
        (lw, a), (rw, b) = bitmap(left, Path(tmp) / "l.bmp", HIGH), bitmap(right, Path(tmp) / "r.bmp", HIGH)
        width, gap = lw + rw + 8, bytes(8 * 3)
        pad = bytes((4 - width * 3 % 4) % 4)
        body = b"".join(a[y] + gap + b[y] + pad for y in range(HIGH - 1, -1, -1))
        head = b"BM" + (54 + len(body)).to_bytes(4, "little") + bytes(4) + (54).to_bytes(4, "little") + (40).to_bytes(4, "little")
        head += width.to_bytes(4, "little") + HIGH.to_bytes(4, "little") + (1).to_bytes(2, "little") + (24).to_bytes(2, "little") + bytes(4) + len(body).to_bytes(4, "little") + bytes(16)
        both = Path(tmp) / "both.bmp"
        both.write_bytes(head + body)
        out.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(["sips", "-s", "format", "png", str(both), "--out", str(out)], check=True, capture_output=True)


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--scenes", type=Path, default=ROOT / "target/scenes")
    ap.add_argument("--write", type=Path)
    ap.add_argument("--pairs", type=Path)
    ap.add_argument("--numbers", action="store_true")
    a = ap.parse_args()
    all_rows = rows(a.scenes)
    print(numbers(all_rows))
    if a.write:
        seen = parity.load(Path(__file__).with_name("seen.json"))
        doc = a.write.read_text()
        if START not in doc or END not in doc:
            sys.exit(f"report: {a.write} lacks the table's two markers")
        head, rest = doc.split(START, 1)
        a.write.write_text(f"{head}{START}\n\n{numbers(all_rows)}.\n\n{table(all_rows, seen)}\n\n{END}{rest.split(END, 1)[1]}")
    if a.pairs:
        for r in all_rows:
            still = STILLS / f"{STILL.get(r['frame'], '')}.png"
            if r["scene"] and r["picture"].is_file() and still.is_file():
                pair(r["picture"], still, a.pairs / f"{r['name']}.png")
    return 0


if __name__ == "__main__":
    sys.exit(main())
