#!/usr/bin/env python3
"""Labelled tile gallery for an orb taste round: one looping APNG per tile, on one page.

Usage: gallery.py <frames dir> <px> <tiles.json> <out.html>
  <frames dir>/<key>.rgba holds a tile's frames, px*px*4 straight-alpha bytes each, as
  `yi_orb::kitty::paint_rgba` writes them. tiles.json is a list of
  {"id": "S3", "key": "thinking", "name": "Thinking", "trigger": "...", "desc": "..."}.
Frames are composited on the terminal ground, 30 fps, looping forever, so a seam in a loop
shows as a jump once per cycle.
"""
import base64, html, json, pathlib, struct, sys, zlib

GROUND = (0x1A, 0x1B, 0x26)


def chunk(kind, data):
    return struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF)


def composite(frame):
    out = bytearray(len(frame) // 4 * 3)
    for j, i in enumerate(range(0, len(frame), 4)):
        a = frame[i + 3] / 255
        for c in range(3):
            out[j * 3 + c] = round(frame[i + c] * a + GROUND[c] * (1 - a))
    return out


def apng(path, px):
    data = path.read_bytes()
    size = px * px * 4
    frames = [composite(data[i : i + size]) for i in range(0, len(data) - size + 1, size)]
    out = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", px, px, 8, 2, 0, 0, 0))
    out += chunk(b"acTL", struct.pack(">II", len(frames), 0))
    seq = 0
    for n, rgb in enumerate(frames):
        rows = b"".join(b"\x00" + bytes(rgb[y * px * 3 : (y + 1) * px * 3]) for y in range(px))
        out += chunk(b"fcTL", struct.pack(">IIIIIHHBB", seq, px, px, 0, 0, 1, 30, 0, 0))
        seq += 1
        body = zlib.compress(rows, 6)
        if n == 0:
            out += chunk(b"IDAT", body)
        else:
            out += chunk(b"fdAT", struct.pack(">I", seq) + body)
            seq += 1
    return "data:image/png;base64," + base64.b64encode(out + chunk(b"IEND", b"")).decode()


def main():
    folder, px, tiles, target = pathlib.Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3], sys.argv[4]
    cards = []
    for tile in json.loads(pathlib.Path(tiles).read_text()):
        esc = {k: html.escape(str(v)) for k, v in tile.items()}
        cards.append(
            f'<div class=card><img src="{apng(folder / (tile["key"] + ".rgba"), px)}">'
            f'<div class=id>{esc["id"]}</div><div class=name>{esc["name"]}</div>'
            f'<div class=dim>{esc.get("trigger", "")}</div><div>{esc.get("desc", "")}</div></div>'
        )
    style = (
        "body{margin:0;padding:24px 16px;background:#1a1b26;color:#c0caf5;font:14px/1.45 system-ui}"
        ".grid{display:grid;grid-template-columns:repeat(auto-fill,minmax(168px,1fr));gap:12px}"
        ".card{background:#20212e;border:1px solid #2c2e40;border-radius:10px;padding:10px;font-size:12px}"
        ".card img{width:100%;aspect-ratio:1;display:block;border-radius:6px}"
        ".id{font:600 13px ui-monospace,monospace;color:#7aa2f7}.name{font-weight:600;font-size:14px}.dim{color:#7982a9}"
    )
    page = f"<!doctype html><meta charset=utf-8><title>Orb gallery</title><style>{style}</style><div class=grid>{''.join(cards)}</div>"
    pathlib.Path(target).write_text(page)
    print(target)


if __name__ == "__main__":
    main()
