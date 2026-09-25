#!/usr/bin/env python3
"""Manifest law (D31). Incidents observed in the reference survey: workspace lints silently
skipped without a per-crate opt-in; two grandfathered folder/crate name mismatches; 84 crates
frozen at 0.1.0 against a root 0.79.1.
Also the crate-root string_slice deny (D109): a root carries the attribute unless the crate is
named in the shrink-only baselines/string_slice_pending.json, and --update may only drop names
from that list."""
import json, sys, tomllib, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

FEATURE_ALLOWLIST = {"yi-cli": {"default", "kernel", "reduce", "tui"}}
PENDING = BASE / "string_slice_pending.json"
DENY = "#![deny(clippy::string_slice)]"


def crate_root(folder):
    lib = ROOT / "crates" / folder / "src/lib.rs"
    return lib if lib.exists() else ROOT / "crates" / folder / "src/main.rs"


manifests = sorted((ROOT / "crates").glob("*/Cargo.toml"))
pending = set(json.loads(PENDING.read_text()))
unlinted = {m.parent.name for m in manifests if DENY not in crate_root(m.parent.name).read_text()}

if "--update" in sys.argv:
    grown = sorted(unlinted - pending)
    if grown:
        print("FAIL string_slice_pending")
        print(f"  {grown} would join the pending list; it only shrinks — restore the attribute")
        print("  and rewrite the slice with .get(..) or a char-boundary walk (D109)")
        sys.exit(1)
    PENDING.write_text(json.dumps(sorted(unlinted), indent=2) + "\n")
    print(f"string_slice pending {sorted(pending)} -> {sorted(unlinted)}")
    sys.exit(0)

errs = []
for m in manifests:
    folder = m.parent.name
    t = tomllib.loads(m.read_text())
    pkg = t.get("package", {})
    name = pkg.get("name", "")
    if name != f"yi-{folder}":
        errs.append(f"{m}: crate {name!r} != yi-{folder} (naming law: folder x -> yi-x)")
    for field in ("version", "edition", "license", "rust-version"):
        v = pkg.get(field)
        if v != {"workspace": True}:
            errs.append(f"{m}: {field} must be {field}.workspace = true")
    if t.get("lints") != {"workspace": True}:
        errs.append(f"{m}: missing [lints] workspace = true (workspace lints are decorative without it)")
    feats = set(t.get("features", {}))
    allowed = FEATURE_ALLOWLIST.get(name, set())
    if feats - allowed:
        errs.append(f"{m}: undeclared features {sorted(feats - allowed)} (13.4 allowlist)")
    for dep, spec in t.get("dependencies", {}).items():
        if not (isinstance(spec, dict) and spec.get("workspace")):
            errs.append(f"{m}: dep {dep} must be {{ workspace = true }} (centralized deps, D31)")
        if name != "yi-types" and dep in ("serde", "serde_derive"):
            errs.append(f"{m}: dep {dep}: serde derives live in yi-types only (.ruler/050-schema.md)")
    if folder in unlinted and folder not in pending:
        errs.append(f"{crate_root(folder).relative_to(ROOT)}: missing {DENY} (a crate root carries it unless the crate is in baselines/string_slice_pending.json, D109)")
for folder in sorted(pending - {m.parent.name for m in manifests}):
    errs.append(f"baselines/string_slice_pending.json: {folder!r} is not a crate")
fail(errs, f"manifests ({len(manifests) - len(pending)} roots deny string_slice, {len(pending)} pending)")
