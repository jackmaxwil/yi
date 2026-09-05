#!/usr/bin/env python3
"""Rasterize provider and lab logos from models.dev into the alpha masks the picker ships.

Every logo models.dev serves is a `currentColor` or stroke-only mark, so the mask is the
whole asset: 32x32 bytes of coverage, tinted with the theme's text colour when the TUI
transmits it. Run once when the set changes; the outputs are committed.

    scripts/logos.py [--from-dir DIR] [--out crates/tui/data/logos] [--px 32]

Needs rsvg-convert (brew install librsvg). PNG decoding is stdlib: zlib + the five filters.
"""
import argparse, hashlib, pathlib, struct, subprocess, sys, urllib.request, zlib

PROVIDERS = ("anthropic", "openai", "openrouter")
LABS = ("google", "qwen", "mistralai", "deepseek", "z-ai", "nvidia", "moonshotai",
        "minimax", "bytedance-seed", "amazon")


def fetch(key, into):
    path = "logos/openrouter.svg" if key == "openrouter" else f"logos/labs/{key}.svg"
    if key in ("anthropic", "openai"):
        path = f"logos/{key}.svg"
    target = into / f"{key}.svg"
    if not target.exists():
        with urllib.request.urlopen(f"https://models.dev/{path}", timeout=20) as r:
            target.write_bytes(r.read())
    return target


def png_rgba(data):
    assert data[:8] == b"\x89PNG\r\n\x1a\n", "not a PNG"
    pos, idat, width, height, depth, ctype = 8, b"", 0, 0, 0, 0
    while pos < len(data):
        length, kind = struct.unpack(">I4s", data[pos:pos + 8])
        body = data[pos + 8:pos + 8 + length]
        if kind == b"IHDR":
            width, height, depth, ctype = struct.unpack(">IIBB", body[:10])
        elif kind == b"IDAT":
            idat += body
        pos += 12 + length
    assert depth == 8 and ctype == 6, f"need 8-bit RGBA, got depth {depth} type {ctype}"
    raw, stride, out, prev = zlib.decompress(idat), width * 4, [], bytearray(width * 4)
    for y in range(height):
        f = raw[y * (stride + 1)]
        line = bytearray(raw[y * (stride + 1) + 1:(y + 1) * (stride + 1)])
        for i in range(stride):
            a = line[i - 4] if i >= 4 else 0
            b = prev[i]
            c = prev[i - 4] if i >= 4 else 0
            if f == 1: line[i] = (line[i] + a) & 255
            elif f == 2: line[i] = (line[i] + b) & 255
            elif f == 3: line[i] = (line[i] + (a + b) // 2) & 255
            elif f == 4:
                p = a + b - c; pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                line[i] = (line[i] + (a if pa <= pb and pa <= pc else b if pb <= pc else c)) & 255
        out.append(bytes(line)); prev = line
    return width, height, b"".join(out)


def main(argv):
    ap = argparse.ArgumentParser()
    ap.add_argument("--from-dir", type=pathlib.Path)
    ap.add_argument("--out", type=pathlib.Path, default=pathlib.Path("crates/tui/data/logos"))
    ap.add_argument("--px", type=int, default=32)
    args = ap.parse_args(argv)
    svgs = args.from_dir or (args.out / "svg")
    svgs.mkdir(parents=True, exist_ok=True); args.out.mkdir(parents=True, exist_ok=True)
    digests = {key: hashlib.md5(fetch(key, svgs).read_bytes()).hexdigest()[:8] for key in PROVIDERS + LABS}
    repeated = {d for d in digests.values() if list(digests.values()).count(d) > 1}
    for key in PROVIDERS + LABS:
        svg, digest = svgs / f"{key}.svg", digests[key]
        if digest in repeated:
            print(f"skip {key:<15} svg {digest} is served for several labs: a placeholder, no identity")
            continue
        png = subprocess.run(["rsvg-convert", "-w", str(args.px), "-h", str(args.px), str(svg)],
                             capture_output=True, check=True).stdout
        w, h, rgba = png_rgba(png)
        assert (w, h) == (args.px, args.px), (key, w, h)
        alpha = bytes(rgba[i + 3] for i in range(0, len(rgba), 4))
        covered = sum(1 for a in alpha if a)
        (args.out / f"{key}.a8").write_bytes(alpha)
        print(f"{key:<15} {covered:4d}/{args.px * args.px} px covered  svg {digest}")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
