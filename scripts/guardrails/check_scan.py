#!/usr/bin/env python3
"""Pinned static scanners, announced offline skips, and a shrink-only finding baseline."""
import argparse
import collections
import concurrent.futures
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, ROOT, fail, test_mask

BASELINE = BASE / "scan.json"
ZIZMOR = "zizmor==1.31.1"
SEMGREP = "semgrep==1.182.1"
FIXTURES = ROOT / "scripts/scan/fixtures"
RULES = ROOT / "scripts/scan/rules"
ZIZMOR_CASES = {
    "unpinned.yml": {"unpinned-uses", "artipacked"},
    "template-injection.yml": {"template-injection"},
    "dangerous-trigger.yml": {"dangerous-triggers"},
}
GITLEAKS_CASES = {
    "planted-gemini.txt": "yi-gemini-api-key",
    "planted-openrouter.txt": "yi-openrouter-api-key",
    "planted-anthropic.txt": "yi-anthropic-api-key",
    "planted-openai.txt": "yi-openai-api-key",
    "planted-github.txt": "yi-github-pat",
    "planted-aws.txt": "yi-aws-access-key",
}
SEMGREP_CASES = {
    "shell_true.py": {"subprocess-shell-true", "yi-subprocess-shell-or-fstring"},
    "tainted_env.py": {"dangerous-subprocess-use-tainted-env-args"},
    "fstring.py": {"dangerous-subprocess-use-tainted-env-args", "yi-subprocess-shell-or-fstring"},
    "os_exec.py": {"dangerous-os-exec-tainted-env-args"},
    "md5.py": {"insecure-hash-algorithm-md5"},
    "sha1.py": {"insecure-hash-algorithm-sha1"},
    "temp_dir.rs": {"yi-rust-temp-dir"},
    "env_command.rs": {"yi-rust-env-command"},
    "from_utf8_unchecked.rs": {"yi-rust-from-utf8-unchecked"},
    "template-injection.yml": {"yi-github-event-run-interpolation"},
}
NOSEMGREP = re.compile(r"nosemgrep:\s*([A-Za-z0-9_.-]+)\s*(.*)")
SUPPRESSION = re.compile(r"zizmor:\s+ignore\[([^]]+)]\s*(.*)")


class Unavailable(Exception):
    pass


def environment():
    env = dict(os.environ)
    env["UV_CACHE_DIR"] = str(ROOT / "target/uv-cache")
    return env


def run(argv, cwd=ROOT):
    return subprocess.run(argv, cwd=cwd, env=environment(), capture_output=True, text=True, check=False)


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


def require_gitleaks():
    executable = shutil.which("gitleaks")
    if executable is None:
        raise Unavailable("gitleaks 8.30.1 is not installed")
    proc = run([executable, "version"])
    if proc.returncode or proc.stdout.strip() != "8.30.1":
        raise Unavailable(f"gitleaks 8.30.1 is required, found {proc.stdout.strip() or 'none'}")
    return executable


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

def semgrep(paths, fixtures=False):
    require_uv(SEMGREP, "semgrep", "1.182.1")
    args = ["scan", "--config", str(RULES / "semgrep"), "--json", "--metrics=off", "--disable-version-check", "--timeout", "30"]
    if not fixtures:
        args += ["--exclude", "scripts/scan/fixtures", "--exclude", "crates/**/tests"]
    proc = run(uv_argv(SEMGREP, "semgrep", [*args, *map(str, paths)]))
    if proc.returncode:
        raise RuntimeError((proc.stdout + proc.stderr).strip())
    try:
        report = json.loads(proc.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"semgrep emitted invalid JSON: {error}") from error
    if report["errors"]:
        raise RuntimeError(json.dumps(report["errors"]))
    findings = []
    for row in report["results"]:
        path, line = row["path"], row["start"]["line"]
        if path.endswith(".rs"):
            source = pathlib.Path(path)
            if not source.is_absolute():
                source = ROOT / source
            mask = test_mask(source.read_text())
            if line <= len(mask) and mask[line - 1]:
                continue
        findings.append(("semgrep", row["check_id"].rsplit(".", 1)[-1], path, line))
    return findings


def shared_ignores():
    text = (ROOT / ".gitguardian.yaml").read_text()
    return re.findall(r'^\s*-\s*"([^"]+)"', text, re.M)

def go_glob(pattern):
    return r"(^|/)" + re.escape(pattern).replace(r"\*\*", ".*").replace(r"\*", "[^/]*") + "$"


def gitleaks_config(ignores, name):
    patterns = [go_glob(pattern) for pattern in shared_ignores()] + [r"(^|/)target/.*$"] if ignores else []
    rows = ",\n".join(f"  '''{pattern}'''" for pattern in patterns)
    path = ROOT / "target" / name
    path.parent.mkdir(parents=True, exist_ok=True)
    text = (RULES / "gitleaks.toml").read_text()
    if patterns:
        text += f"\n[[allowlists]]\npaths = [\n{rows}\n]\n"
    path.write_text(text)
    return path


def gitleaks(mode, config, tool="gitleaks", repo=ROOT):
    executable = require_gitleaks()
    args = [mode, "--no-banner", "--no-color", "--redact", "--config", str(config),
            "--report-format", "json", "--report-path", "-"]
    if mode == "git":
        args += ["--log-opts", "origin/main..HEAD"] if tool == "gitleaks" else []
        args.append(str(repo))
    else:
        args.append(str(repo))
    proc = run([executable, *args], repo)
    if proc.returncode not in (0, 1):
        raise RuntimeError((proc.stdout + proc.stderr).strip())
    try:
        rows = json.loads(proc.stdout)
    except json.JSONDecodeError as error:
        raise RuntimeError(f"gitleaks emitted invalid JSON: {error}: {proc.stderr.strip()}") from error
    findings = []
    for row in rows:
        path = pathlib.Path(row["File"])
        try:
            name = str(path.relative_to(repo))
        except ValueError:
            name = str(path)
        findings.append((tool, row["RuleID"], name, row["StartLine"]))
    return findings


def suppressions():
    out = []
    for path in sorted((ROOT / ".forgejo/workflows").glob("*.yml")):
        for number, line in enumerate(path.read_text().splitlines(), 1):
            match = SUPPRESSION.search(line)
            if match:
                for rule in match.group(1).split(","):
                    out.append(("zizmor", rule, str(path.relative_to(ROOT)), number, match.group(2)))
    for base in ("scripts", "evals", "python", "crates", ".forgejo"):
        for path in (ROOT / base).rglob("*"):
            if not path.is_file() or path.suffix not in (".py", ".rs", ".yml", ".yaml") or FIXTURES in path.parents:
                continue
            for number, line in enumerate(path.read_text(errors="ignore").splitlines(), 1):
                match = NOSEMGREP.search(line)
                if match:
                    out.append(("semgrep", match.group(1), str(path.relative_to(ROOT)), number, match.group(2)))
    out += [("gitleaks", "paths-ignore", pattern, 0, "shared .gitguardian.yaml path exclusion")
            for pattern in shared_ignores()]
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


def save_baseline(findings, tools):
    old = load_baseline() if BASELINE.is_file() else {}
    active = {key: value for key, value in old.items() if key[0] in tools}
    new_tools = tools - {tool for tool, _ in old}
    checked = [row for row in findings if row[0] not in new_tools]
    errors, _ = baseline_problems(checked, active)
    if errors:
        fail(errors + ["refusing to grow the scan baseline"], "scan --update")
    counts = collections.Counter((tool, rule) for tool, rule, _, _ in findings)
    for key in old:
        counts.setdefault(key, 0)
    ordered = sorted([list(key), count] for key, count in counts.items())


def selfcheck(off="", fixtures=None):
    fixtures = fixtures or {}
    errors = []
    zizmor_findings = list(fixtures.get("zizmor", []))
    if off in {rule for rules in ZIZMOR_CASES.values() for rule in rules}:
        zizmor_findings = [row for row in zizmor_findings if row[1] != off]
    expected = {(path, rule) for path, rules in ZIZMOR_CASES.items() for rule in rules}
    actual = {(pathlib.Path(path).name, rule) for _, rule, path, _ in zizmor_findings}
    if "zizmor" in fixtures:
        for path, rule in sorted(expected - actual):
            errors.append(f"zizmor fixture {path} produced no {rule} finding")
        if any(pathlib.Path(path).name == "clean.yml" for _, _, path, _ in zizmor_findings):
            errors.append("zizmor clean fixture produced a finding")
    gitleaks_findings = list(fixtures.get("gitleaks", []))
    if off in set(GITLEAKS_CASES.values()):
        gitleaks_findings = [row for row in gitleaks_findings if row[1] != off]
    actual = {(pathlib.Path(path).name, rule) for _, rule, path, _ in gitleaks_findings}
    if "gitleaks" in fixtures:
        for path, rule in sorted(set(GITLEAKS_CASES.items()) - actual):
            errors.append(f"gitleaks fixture {path} produced no {rule} finding")
        if any(pathlib.Path(path).name == "clean.txt" for _, _, path, _ in gitleaks_findings):
            errors.append("gitleaks clean fixture produced a finding")
    semgrep_findings = list(fixtures.get("semgrep", []))
    if off in {rule for rules in SEMGREP_CASES.values() for rule in rules}:
        semgrep_findings = [row for row in semgrep_findings if row[1] != off]
    expected = {(path, rule) for path, rules in SEMGREP_CASES.items() for rule in rules}
    actual = {(pathlib.Path(path).name, rule) for _, rule, path, _ in semgrep_findings}
    if "semgrep" in fixtures:
        for path, rule in sorted(expected - actual):
            errors.append(f"semgrep fixture {path} produced no {rule} finding")
        if any(pathlib.Path(path).name in ("clean.py", "clean.rs") for _, _, path, _ in semgrep_findings):
            errors.append("semgrep clean fixture produced a finding")
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
    if off == "ignores" or go_glob("**/fixtures/**/planted*") == r"^$":
        errors.append("shared gitleaks ignores are not converted to regular expressions")
    if not SUPPRESSION.search("uses: x/y@v1 # zizmor: ignore[unpinned-uses] mirror SHA is unverified"):
        errors.append("a same-line zizmor suppression was not counted")
    return errors


def unavailable(error, strict):
    row = f"skip scan ({error})"
    if strict:
        fail([row], "scan")
    print(row)


def gather(tasks, strict):
    findings, tools = [], set()
    with concurrent.futures.ThreadPoolExecutor(max_workers=len(tasks)) as pool:
        futures = {name: pool.submit(call) for name, call in tasks.items()}
        for name, future in futures.items():
            try:
                rows = future.result()
            except Unavailable as error:
                unavailable(f"{name}: {error}", strict)
                continue
            tools.add(rows[0][0] if rows else name.split("-", 1)[0])
            findings += rows
    return findings, tools


def normal_tasks(history=False):
    if history:
        config = gitleaks_config(True, "gitleaks-history.toml")
        return {"gitleaks-history": lambda: gitleaks("git", config, "gitleaks-history")}
    config = gitleaks_config(True, "gitleaks.toml")
    return {
        "zizmor": lambda: zizmor(sorted((ROOT / ".forgejo/workflows").glob("*.yml"))),
        "gitleaks-tree": lambda: gitleaks("dir", config),
        "gitleaks-diff": lambda: gitleaks("git", config),
        "semgrep": lambda: semgrep(["evals", "scripts", "python", ".forgejo", "crates"]),
    }


def fixture_tasks():
    config = gitleaks_config(False, "gitleaks-fixtures.toml")
    return {
        "zizmor": lambda: zizmor(sorted((FIXTURES / "zizmor").glob("*.yml"))),
        "gitleaks": lambda: gitleaks("dir", config, repo=FIXTURES / "gitleaks"),
        "semgrep": lambda: semgrep([FIXTURES / "semgrep", FIXTURES / "zizmor/template-injection.yml"], True),
    }


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--disable")
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--history", action="store_true")
    args = parser.parse_args(argv)
    strict = bool(os.environ.get("CI"))
    if args.selfcheck or args.disable:
        findings, tools = gather(fixture_tasks(), strict)
        fixtures = {tool: [row for row in findings if row[0] == tool] for tool in tools}
        if args.disable:
            fail(selfcheck(args.disable, fixtures), f"scan selfcheck with {args.disable} disabled")
        errors = selfcheck(fixtures=fixtures)
        checks = {rule for rules in ZIZMOR_CASES.values() for rule in rules}
        checks |= {rule for rules in SEMGREP_CASES.values() for rule in rules}
        checks |= set(GITLEAKS_CASES.values()) | {"baseline", "ignores"}
        for off in sorted(checks):
            if not selfcheck(off, fixtures):
                errors.append(f"selfcheck passes with the {off} check disabled")
        fail(errors, "scan selfcheck")
        return
    findings, tools = gather(normal_tasks(args.history), strict)
    if args.update:
        save_baseline(findings, tools)
        print(f"ok   scan --update ({BASELINE.relative_to(ROOT)})")
        return
    try:
        baseline = load_baseline()
    except Unavailable as error:
        unavailable(error, strict)
        return
    active = {key: value for key, value in baseline.items() if key[0] in tools}
    errors, hints = baseline_problems(findings, active)
    for tool, rule, path, line, reason in suppressions():
        if not reason:
            errors.append(f"{path}:{line}: {tool}/{rule} suppression has no reason")
        else:
            location = f"{path}:{line}" if line else path
            print(f"suppressed {tool}/{rule} {location}: {reason}")
    for hint in hints:
        print(hint)
    fail(errors, f"scan ({len(findings)} findings)")


if __name__ == "__main__":
    main(sys.argv[1:])
