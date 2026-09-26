#!/usr/bin/env python3
"""Schema lock (design §20): every serialized shape lives in yi-types; this locks their
normalized definitions so a schema edit is a reviewed schemas.lock diff, never a silent
drift. Regenerate deliberately with --update."""
import json, pathlib, re, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

LOCK = BASE / "schemas.lock"
TYPES_SRC = ROOT / "crates/types/src"

def normalize(block):
    block = re.sub(r"//[^\n]*", "", block)
    block = re.sub(r"\s+", " ", block).strip()
    return block

def extract(path):
    text = path.read_text()
    text = re.sub(r"///[^\n]*\n", "", text)
    shapes = {}
    pattern = re.compile(
        r"((?:#\[[^\]]*\]\s*)*)pub (?:struct|enum) (\w+)", re.S
    )
    for match in pattern.finditer(text):
        name = match.group(2)
        start = match.start()
        brace = text.find("{", match.end())
        semi = text.find(";", match.end())
        if brace == -1 or (semi != -1 and semi < brace):
            end = semi + 1
        else:
            depth = 0
            end = brace
            for index in range(brace, len(text)):
                if text[index] == "{":
                    depth += 1
                elif text[index] == "}":
                    depth -= 1
                    if depth == 0:
                        end = index + 1
                        break
        shapes[f"{path.stem}::{name}"] = normalize(text[start:end])
    return shapes

def current():
    shapes = {}
    for path in sorted(TYPES_SRC.glob("**/*.rs")):
        shapes.update(extract(path))
    return shapes

def main():
    shapes = current()
    if "--update" in sys.argv:
        LOCK.write_text(json.dumps(shapes, indent=1, sort_keys=True) + "\n")
        print(f"ok   schemas_lock (baseline updated: {len(shapes)} shapes)")
        return
    locked = json.loads(LOCK.read_text() or "{}")
    errs = []
    for name in sorted(set(locked) - set(shapes)):
        errs.append(f"removed shape: {name} (schema removal is a breaking change; --update deliberately)")
    for name in sorted(set(shapes) - set(locked)):
        errs.append(f"new shape not locked: {name} (run --update in its own commit)")
    for name in sorted(set(shapes) & set(locked)):
        if shapes[name] != locked[name]:
            errs.append(f"shape changed: {name} (review the diff, then --update in its own commit)")
    fail(errs, f"schemas_lock ({len(shapes)} shapes)")

main()
