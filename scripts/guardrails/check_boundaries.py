#!/usr/bin/env python3
"""Crate-boundary allowlist. Incident: jcode's denylist covered 14/85 crates, omitted its three
100k-line crates, and carried two dead names — allowlist, unknown = error, stale = error."""
import sys, tomllib, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

allow = tomllib.loads((ROOT / "scripts/guardrails/boundaries.toml").read_text())
errs = []
seen = set()
for m in sorted((ROOT / "crates").glob("*/Cargo.toml")):
    t = tomllib.loads(m.read_text())
    name = t["package"]["name"]
    seen.add(name)
    if name not in allow:
        errs.append(f"{name}: not in boundaries.toml (unknown crate = error)")
        continue
    deps = {d for d in t.get("dependencies", {}) if d.startswith("yi-")}
    extra = deps - set(allow[name])
    if extra:
        errs.append(f"{name}: undeclared internal deps {sorted(extra)}")
for name in set(allow) - seen:
    errs.append(f"boundaries.toml: stale entry {name} (stale name = error)")
fail(errs, "boundaries")
