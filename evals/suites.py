#!/usr/bin/env python3
"""The benchmark suite registry (evals/suites.json): every suite a gate or a refine round may run.

    python3 evals/suites.py --selfcheck
    python3 evals/suites.py field <suite> <key>      # one value, for a driver to read
    python3 evals/suites.py tasks <suite> <group>    # one task id a line

A Harbor row names its dataset by digest (E13, generalized: `@latest` would move under a row), no
task sits in two groups of one suite, and a suite with no calibration (its A/A run, which fixes the
spread a verdict is judged against) is refused wherever a gate would read it. A new Harbor dataset
is one row: pin its digest, list the tasks this host can run, run an A/A, record it.
"""
import json, pathlib, sys

ROOT = pathlib.Path(__file__).resolve().parent
REGISTRY = ROOT / "suites.json"
HARNESSES = ("harbor", "inner", "pier")


class Refused(Exception):
    pass


def load(path=REGISTRY):
    return json.loads(pathlib.Path(path).read_text())


def problems(registry):
    out = []
    for name, suite in registry.get("suites", {}).items():
        if suite.get("harness") not in HARNESSES:
            out.append(f"{name}: harness {suite.get('harness')!r} is not one of {', '.join(HARNESSES)}")
        dataset = suite.get("dataset")
        if suite.get("harness") in ("harbor", "pier") and dataset is not None and "@sha256:" not in dataset:
            out.append(f"{name}: dataset {dataset!r} is not pinned by digest")
        seen = {}
        for group, tasks in (suite.get("groups") or {}).items():
            for task in tasks:
                if task in seen:
                    out.append(f"{name}: {task} is in both {seen[task]} and {group}")
                seen[task] = group
    for name, cascade in registry.get("cascades", {}).items():
        for stage, suite in cascade.items():
            if suite not in registry.get("suites", {}):
                out.append(f"cascade {name}: {stage} names no suite {suite!r}")
    return out


def gateable(registry, name):
    """The suite, refused if no gate can read it: unknown, unpinned or uncalibrated."""
    suite = registry.get("suites", {}).get(name)
    if suite is None:
        raise Refused(f"no suite {name!r} in evals/suites.json")
    if suite.get("harness") in ("harbor", "pier") and not suite.get("dataset"):
        raise Refused(f"{name}: no dataset is pinned")
    if not suite.get("calibration"):
        raise Refused(f"{name}: uncalibrated; run its A/A and record it before a gate reads it")
    return suite


def selfcheck():
    registry = load()
    found = problems(registry)
    tbv4 = registry["suites"]["tbv4"]["dataset"]
    baseline = (ROOT / "drivers/tbv4_baseline.sh").read_text()
    if f'DATASET="{tbv4}"' not in baseline:
        found.append("tbv4's digest differs from drivers/tbv4_baseline.sh's DATASET")
    broken = {"suites": {"x": {"harness": "harbor", "dataset": "a/b@latest", "groups": {"a": ["t"], "b": ["t"]}}}}
    if len(problems(broken)) != 2:
        found.append("an unpinned dataset and a task in two groups are not both refused")
    for name, why in (("swe-atlas-qna", "pinned"), ("tbv4", "uncalibrated"), ("inner-economy", None)):
        try:
            gateable(registry, name)
            if why:
                found.append(f"{name} passed the gate check though it is not {why}")
        except Refused as refusal:
            if not why or why not in str(refusal):
                found.append(f"{name}: {refusal}")
    return found


def main(argv=None):
    args = (argv if argv is not None else sys.argv[1:]) or ["--selfcheck"]
    if args[0] == "--selfcheck":
        found = selfcheck()
        for line in found:
            print(line, file=sys.stderr)
        print("ok   suites" if not found else "FAIL suites")
        return 1 if found else 0
    registry = load()
    suite = registry["suites"].get(args[1]) if len(args) > 1 else None
    if suite is None:
        print(f"refused: no suite {args[1:2]}", file=sys.stderr)
        return 2
    if args[0] == "field" and len(args) == 3:
        value = suite.get(args[2])
        print(value if isinstance(value, str) else json.dumps(value))
        return 0
    if args[0] == "tasks" and len(args) == 3:
        print("\n".join(suite.get("groups", {}).get(args[2], [])))
        return 0
    print("usage: suites.py --selfcheck | field <suite> <key> | tasks <suite> <group>", file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main())
