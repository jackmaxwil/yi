#!/usr/bin/env python3
"""Crate-boundary allowlist. Incident: a surveyed denylist covered 14/85 crates, omitted its three
100k-line crates, and carried two dead names — allowlist, unknown = error, stale = error.
Dev-dependencies are edges too: yi-loop's tests reached yi-ai and yi-context while only
`[dependencies]` was read, so a test-only edge is listed in the `[dev]` table or refused."""
import copy, sys, tomllib, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail


def manifests(root):
    return {t["package"]["name"]: t for t in (tomllib.loads(m.read_text()) for m in sorted(root.glob("crates/*/Cargo.toml")))}

def dev_dependencies(t):
    found = set()
    tables = [t.get("dev-dependencies", {})]
    tables += [target.get("dev-dependencies", {}) for target in t.get("target", {}).values()]
    for table in tables:
        for name, spec in table.items():
            package = spec.get("package", name) if isinstance(spec, dict) else name
            if package.startswith("yi-"):
                found.add(package)
    return found

def errors(crates, allow, off=""):
    allow = dict(allow)
    dev = allow.pop("dev", {})
    errs = []
    for name, t in sorted(crates.items()):
        if name not in allow:
            if off != "unknown":
                errs.append(f"{name}: not in boundaries.toml (unknown crate = error)")
            continue
        deps = {d for d in t.get("dependencies", {}) if d.startswith("yi-")}
        extra = deps - set(allow[name])
        if extra and off != "deps":
            errs.append(f"{name}: undeclared internal deps {sorted(extra)}")
        tests = set() if off == "dev" else dev_dependencies(t)
        extra = tests - set(allow[name]) - set(dev.get(name, []))
        if extra:
            errs.append(f"{name}: undeclared internal dev-deps {sorted(extra)} (list them under [dev])")
        stale = set(dev.get(name, [])) - tests
        if stale and off not in ("dev", "dev-stale"):
            errs.append(f"boundaries.toml [dev]: stale edge {name} -> {sorted(stale)} (stale edge = error)")
    if off != "stale":
        errs += [f"boundaries.toml: stale entry {name} (stale name = error)" for name in sorted(set(allow) - set(crates))]
    if off != "dev-stale-crate":
        errs += [f"boundaries.toml [dev]: stale entry {name} (stale name = error)" for name in sorted(set(dev) - set(crates))]
    return errs


def selfcheck(off=""):
    """Each rule, against the real manifests and allowlist with one planted defect."""
    crates = manifests(ROOT)
    allow = tomllib.loads((ROOT / "scripts/guardrails/boundaries.toml").read_text())
    deps = copy.deepcopy(crates)
    target_dev = copy.deepcopy(crates)
    target_dev["yi-types"]["target"] = {"cfg(unix)": {"dev-dependencies": {"yi-ai": {"workspace": True}}}}
    renamed_dev = copy.deepcopy(crates)
    renamed_dev["yi-types"]["dev-dependencies"] = {"ai_alias": {"package": "yi-ai", "workspace": True}}
    declared = {**allow, "dev": {**allow["dev"], "yi-types": ["yi-ai"]}}
    deps["yi-types"].setdefault("dependencies", {})["yi-runtime"] = {"workspace": True}
    cases = {
        "unknown crate": ({**crates, "yi-new": {"package": {"name": "yi-new"}}}, allow, "yi-new: not in"),
        "undeclared dep": (deps, allow, "yi-types: undeclared internal deps ['yi-runtime']"),
        "unlisted dev edge": (crates, {**allow, "dev": {**allow["dev"], "yi-loop": ["yi-context"]}},
                              "yi-loop: undeclared internal dev-deps ['yi-ai']"),
        "stale dev edge": (crates, {**allow, "dev": {**allow["dev"], "yi-cli": ["yi-tools", "yi-kernel"]}},
                           "stale edge yi-cli -> ['yi-kernel']"),
        "stale crate": (crates, {**allow, "yi-gone": []}, "stale entry yi-gone"),
        "target-specific dev edge": (target_dev, allow, "yi-types: undeclared internal dev-deps ['yi-ai']"),
        "renamed dev edge": (renamed_dev, allow, "yi-types: undeclared internal dev-deps ['yi-ai']"),
        "declared target-specific dev edge": (target_dev, declared, None),
        "declared renamed dev edge": (renamed_dev, declared, None),
        "stale dev crate": (crates, {**allow, "dev": {**allow["dev"], "yi-gone": []}},
                            "boundaries.toml [dev]: stale entry yi-gone"),
    }
    errs = [f"real tree: {e}" for e in errors(crates, allow, off)]
    for case, (c, a, want) in cases.items():
        found = errors(c, a, off)
        if want is None:
            if found:
                errs.append(f"{case}: gate said {found}, not no errors")
        elif len(found) != 1 or want not in found[0]:
            errs.append(f"{case}: gate said {found}, not one line naming {want!r}")
    return errs


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        errs = selfcheck()
        for off in ("unknown", "deps", "dev", "dev-stale", "dev-stale-crate", "stale"):
            if not selfcheck(off):
                errs.append(f"selfcheck passes with the {off} mutant, so it refutes nothing")
        fail(errs, "boundaries selfcheck")
        sys.exit(0)
    fail(errors(manifests(ROOT), tomllib.loads((ROOT / "scripts/guardrails/boundaries.toml").read_text())), "boundaries")
