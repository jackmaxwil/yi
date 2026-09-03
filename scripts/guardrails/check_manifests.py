#!/usr/bin/env python3
"""Manifest law (D31). Incidents observed in the reference survey: workspace lints silently
skipped without a per-crate opt-in; two grandfathered folder/crate name mismatches; 84 crates
frozen at 0.1.0 against a root 0.79.1."""
import sys, tomllib, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

FEATURE_ALLOWLIST = {"yi-cli": {"default", "kernel", "reduce", "tui"}}
errs = []
for m in sorted((ROOT / "crates").glob("*/Cargo.toml")):
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
fail(errs, "manifests")
