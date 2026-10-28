#!/usr/bin/env python3
"""Pinned static scanners, announced offline skips, and a shrink-only finding baseline."""
import argparse
import collections
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, ROOT, fail

BASELINE = BASE / "scan.json"
ZIZMOR = "zizmor==1.31.1"
FIXTURES = ROOT / "scripts/scan/fixtures"
ZIZMOR_CASES = {
    "unpinned.yml": {"unpinned-uses", "artipacked"},
    "template-injection.yml": {"template-injection"},
    "dangerous-trigger.yml": {"dangerous-triggers"},
}
SUPPRESSION = re.compile(r"zizmor:\s+ignore\[([^]]+)]\s*(.*)")


class Unavailable(Exception):
    pass


def environment():
    env = dict(os.environ)
    env.setdefault("UV_CACHE_DIR", str(ROOT / "target/uv-cache"))
    return env


def run(argv):
    return subprocess.run(argv, cwd=ROOT, env=environment(), capture_output=True, text=True, check=False)


def uv_argv(package, executable, args):
    offline = [] if os.environ.get("CI") else ["--offline"]
    return ["uvx", *offline, "--from", package, executable, *args]


def require_uv(package, executable, version):
    if shutil.which("uv") is None:
        raise Unavailable("uv is not installed")
    proc = run(uv_argv(package, executable, ["--version"]))
    if proc.returncode or version not in proc.stdout:
        reason = (proc.stdout + proc.stderr).strip().splitlines()
        raise Unavailable(reason[-1] if reason else f"{executable} {version} is unavailable")


def zizmor(paths):
    require_uv(ZIZMOR, "zizmor", "1.31.1")
    args = ["--no-online-audits", "--no-config", "--format", "json", *map(str, paths)]
    proc = run(uv_argv(ZIZMOR, "zizmor", args))
    if proc.returncode not in (0, 14):
        raise RuntimeError((proc.stdout + proc.stderr).strip())
    try:
        rows = json.loads(proc.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"zizmor emitted invalid JSON: {error}") from error
    findings = []
    for row in rows:
        locations = row["locations"]
        location = next((item for item in locations if item["symbolic"].get("kind") == "Primary"), locations[0])
        path = location["symbolic"]["key"]["Local"]["verbatim_path"]
        line = location["concrete"]["location"]["start_point"]["row"] + 1
        findings.append(("zizmor", row["ident"], path, line))
    return findings


def suppressions():
    out = []
    for path in sorted((ROOT / ".forgejo/workflows").glob("*.yml")):
        for number, line in enumerate(path.read_text().splitlines(), 1):
            match = SUPPRESSION.search(line)
            if match:
                out.append(("zizmor", match.group(1), str(path.relative_to(ROOT)), number, match.group(2)))
    return out


def baseline_problems(findings, baseline, off=""):
    counts = collections.Counter((tool, rule) for tool, rule, _, _ in findings)
    errors, hints = [], []
    if off != "baseline":
        for key in sorted(set(counts) | set(baseline)):
            current, allowed = counts.get(key, 0), baseline.get(key)
            label = "/".join(key)
            if allowed is None:
                errors.append(f"new rule id {label}: {current} finding(s)")
            elif current > allowed:
                errors.append(f"{label}: {current} findings above baseline {allowed}")
            elif current < allowed:
                hints.append(f"ratchet: {label} {allowed} -> {current}")
        violated = {(tool, rule) for tool, rule, _, _ in findings
                    if (tool, rule) not in baseline or counts[tool, rule] > baseline[tool, rule]}
        errors += [f"{path}:{line}: {tool}/{rule}" for tool, rule, path, line in findings
                   if (tool, rule) in violated]
    return errors, hints


def load_baseline():
    if not BASELINE.is_file():
        raise Unavailable(f"{BASELINE.relative_to(ROOT)} is not seeded")
    raw = json.loads(BASELINE.read_text())
    return {(tool, rule): count for (tool, rule), count in raw}


def save_baseline(findings):
    old = load_baseline() if BASELINE.is_file() else {}
    errors, _ = baseline_problems(findings, old)
    new_tools = {tool for tool, _, _, _ in findings} - {tool for tool, _ in old}
    errors = [error for error in errors if error.split("/", 1)[0] not in new_tools]
    if errors:
        fail(errors + ["refusing to grow the scan baseline"], "scan --update")
    counts = collections.Counter((tool, rule) for tool, rule, _, _ in findings)
    for key in old:
        counts.setdefault(key, 0)
    ordered = sorted((*key, count) for key, count in counts.items())
    BASELINE.write_text(json.dumps(ordered, indent=2) + "\n")


def fixture_zizmor():
    paths = sorted((FIXTURES / "zizmor").glob("*.yml"))
    return zizmor(paths)


def selfcheck(off="", fixture_results=None):
    errors = []
    findings = list(fixture_results or [])
    expected = {(path, rule) for path, rules in ZIZMOR_CASES.items() for rule in rules}
    if off in {rule for rules in ZIZMOR_CASES.values() for rule in rules}:
        findings = [row for row in findings if row[1] != off]
    actual = {(pathlib.Path(path).name, rule) for _, rule, path, _ in findings}
    for path, rule in sorted(expected - actual):
        errors.append(f"zizmor fixture {path} produced no {rule} finding")
    if any(pathlib.Path(path).name == "clean.yml" for _, _, path, _ in findings):
        errors.append("zizmor clean fixture produced a finding")
    sample = [("zizmor", "new", "a.yml", 3), ("zizmor", "old", "b.yml", 4)]
    baseline = {("zizmor", "old"): 1, ("zizmor", "shrinking"): 2}
    problems, hints = baseline_problems(sample, baseline, off)
    if off != "baseline":
        if not any("new rule id" in error for error in problems):
            errors.append("baseline accepted a new rule id")
        if not any("a.yml:3" in error for error in problems):
            errors.append("baseline failure omitted file:line")
        if hints != ["ratchet: zizmor/shrinking 2 -> 0"]:
            errors.append(f"baseline shrink hint was {hints}")
    if off == "baseline" and not problems and not hints:
        errors.append("disabled baseline checks accepted a new rule id and a shrink")
    if not SUPPRESSION.search("uses: x/y@v1 # zizmor: ignore[unpinned-uses] mirror SHA is unverified"):
        errors.append("a same-line zizmor suppression was not counted")
    return errors


def unavailable(error, strict):
    row = f"skip scan ({error})"
    if strict:
        fail([row], "scan")
    print(row)


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--disable")
    parser.add_argument("--update", action="store_true")
    args = parser.parse_args(argv)
    strict = bool(os.environ.get("CI"))
    if args.selfcheck or args.disable:
        try:
            fixtures = fixture_zizmor()
        except Unavailable as error:
            unavailable(error, strict)
            return
        if args.disable:
            fail(selfcheck(args.disable, fixtures), f"scan selfcheck with {args.disable} disabled")
        errors = selfcheck(fixture_results=fixtures)
        checks = {rule for rules in ZIZMOR_CASES.values() for rule in rules} | {"baseline"}
        for off in sorted(checks):
            if not selfcheck(off, fixtures):
                errors.append(f"selfcheck passes with the {off} check disabled")
        fail(errors, "scan selfcheck")
        return
    try:
        findings = zizmor(sorted((ROOT / ".forgejo/workflows").glob("*.yml")))
    except Unavailable as error:
        unavailable(error, strict)
        return
    if args.update:
        save_baseline(findings)
        print(f"ok   scan --update ({BASELINE.relative_to(ROOT)})")
        return
    try:
        baseline = load_baseline()
    except Unavailable as error:
        unavailable(error, strict)
        return
    errors, hints = baseline_problems(findings, baseline)
    for tool, rule, path, line, reason in suppressions():
        if not reason:
            errors.append(f"{path}:{line}: {tool}/{rule} suppression has no reason")
        else:
            print(f"suppressed {tool}/{rule} {path}:{line}: {reason}")
    for hint in hints:
        print(hint)
    fail(errors, f"scan ({len(findings)} findings)")


if __name__ == "__main__":
    main(sys.argv[1:])
