#!/usr/bin/env python3
"""Pinned static scanners, announced offline skips, and a shrink-only finding baseline."""
import argparse
import fnmatch
import collections
import concurrent.futures
import contextlib
import io
import json
import os
import pathlib
import re
import shutil
import subprocess
import sys
import tempfile

sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import BASE, ROOT, fail, test_mask
from check_public_surface import unscanned

BASELINE = BASE / "scan.json"
GITLEAKS = (ROOT / "scripts/scan/gitleaks.version").read_text().strip()
ZIZMOR = "zizmor==1.31.1"
SEMGREP = "semgrep==1.182.1"
FIXTURES = ROOT / "scripts/scan/fixtures"
RULES = ROOT / "scripts/scan/rules"
SCAN_ROOTS = ("evals", "scripts", "python", ".forgejo", "crates")
ZIZMOR_CASES = {
    "unpinned.yml": {"unpinned-uses", "artipacked"},
    "template-injection.yml": {"template-injection"},
    "dangerous-trigger.yml": {"dangerous-triggers"},
}
GITLEAKS_CASES = {
    "planted-gemini.txt": {"yi-gemini-api-key"},
    "planted-openrouter.txt": {"yi-openrouter-api-key"},
    "planted-anthropic.txt": {"yi-anthropic-api-key"},
    "planted-openai.txt": {"yi-openai-api-key"},
    "planted-github.txt": {"yi-github-pat"},
    "planted-aws.txt": {"yi-aws-access-key"},
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
}
FAMILIES = {
    "zizmor": (ZIZMOR_CASES, {"clean.yml"}),
    "gitleaks": (GITLEAKS_CASES, {"clean.txt"}),
    "semgrep": (SEMGREP_CASES, {"clean.py", "clean.rs"}),
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
        raise Unavailable(f"gitleaks {GITLEAKS} is not installed")
    proc = run([executable, "version"])
    if proc.returncode or proc.stdout.strip() != GITLEAKS:
        raise Unavailable(f"gitleaks {GITLEAKS} is required, found {proc.stdout.strip() or 'none'}")
    return executable


def zizmor(paths):
    require_uv(ZIZMOR, "zizmor", ZIZMOR.split("==", 1)[1])
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
    require_uv(SEMGREP, "semgrep", SEMGREP.split("==", 1)[1])
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


def go_glob(pattern):
    translated = fnmatch.translate(pattern)
    translated = re.sub(r"\(\?=\(\?P<g\d+>(.*?)\)\)\(\?P=g\d+\)", r"\1", translated)
    return translated.replace(r"\Z", r"\z")


def gitleaks_config(ignores, name):
    patterns = [go_glob(pattern) for pattern in unscanned()] + [r"(^|/)target/.*$"] if ignores else []
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
    for base in SCAN_ROOTS:
        for path in (ROOT / base).rglob("*"):
            if not path.is_file() or path.suffix not in (".py", ".rs", ".yml", ".yaml") or FIXTURES in path.parents:
                continue
            text = path.read_text(errors="ignore")
            for match in NOSEMGREP.finditer(text):
                number = text.count("\n", 0, match.start()) + 1
                out.append(("semgrep", match.group(1), str(path.relative_to(ROOT)), number, match.group(2)))
    out += [("gitleaks", "paths-ignore", pattern, 0, "shared .gitguardian.yaml path exclusion")
            for pattern in unscanned()]
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


def save_baseline(findings, tools, off=""):
    old = load_baseline() if BASELINE.is_file() else {}
    active = {key: value for key, value in old.items() if key[0] in tools}
    new_tools = tools - {tool for tool, _ in old}
    checked = [row for row in findings if row[0] not in new_tools]
    errors, _ = baseline_problems(checked, active)
    if errors and off != "update":
        fail(errors + ["refusing to grow the scan baseline"], "scan --update")
    counts = {key: value for key, value in old.items() if key[0] not in tools}
    counts.update(collections.Counter((tool, rule) for tool, rule, _, _ in findings))
    for key in active:
        counts.setdefault(key, 0)
    ordered = sorted([list(key), count] for key, count in counts.items())
    BASELINE.write_text(json.dumps(ordered, indent=2) + "\n")


def repository_fixtures():
    with tempfile.TemporaryDirectory() as tmp:
        repo = pathlib.Path(tmp)
        subprocess.run(["git", "init", "-q", tmp], check=True)
        key = "AKIA" + "J7K2Q9M4X8C6V3P5"
        (repo / ".env").write_text(f'AWS_ACCESS_KEY_ID = "{key}"\n')
        subprocess.run(["git", "-C", tmp, "add", ".env"], check=True)
        subprocess.run(["git", "-C", tmp, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "base"], check=True)
        subprocess.run(["git", "-C", tmp, "update-ref", "refs/remotes/origin/main", "HEAD"], check=True)
        key = "AKIA" + "Q9M4X8C6V3P5J7K2"
        (repo / ".env").write_text(f'AWS_ACCESS_KEY_ID = "{key}"\n')
        subprocess.run(["git", "-C", tmp, "add", ".env"], check=True)
        subprocess.run(["git", "-C", tmp, "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "branch"], check=True)
        config = gitleaks_config(False, "gitleaks-repository-fixtures.toml")
        return {
            "history": gitleaks("git", config, "gitleaks-history", repo=repo),
            "diff": gitleaks("git", config, repo=repo),
        }


def selfcheck(off="", fixtures=None):
    global BASELINE
    fixtures = fixtures or {}
    errors = []
    for tool, (cases, clean_names) in FAMILIES.items():
        findings = [row for row in fixtures.get(tool, []) if row[1] != off]
        expected = {(path, rule) for path, rules in cases.items() for rule in rules}
        actual = {(pathlib.Path(path).name, rule) for _, rule, path, _ in findings}
        if tool in fixtures:
            for path, rule in sorted(expected - actual):
                errors.append(f"{tool} fixture {path} produced no {rule} finding")
            if any(pathlib.Path(path).name in clean_names for _, _, path, _ in findings):
                errors.append(f"{tool} clean fixture produced a finding")
    origin_jobs = {
        ("autofix.yml", "autofix"),
        ("mirror.yml", "mirror"),
        ("pr.yml", "title"),
        ("pr.yml", "live"),
        ("review.yml", "review"),
    }
    if off == "credentials":
        origin_jobs = {("probe.yml", "probe")}
    for workflow, job in sorted(origin_jobs):
        text = (ROOT / ".forgejo/workflows" / workflow).read_text() if off != "credentials" else (
            "jobs:\n  probe:\n    steps:\n      - uses: actions/checkout@v4\n"
            "        with:\n          persist-credentials: false\n"
        )
        parts = text.split(f"\n  {job}:\n", 1)
        if len(parts) != 2:
            errors.append(f"workflow audit could not find {workflow}:{job}")
        elif "persist-credentials: false" in re.split(r"(?m)^  \S", parts[1], maxsplit=1)[0]:
            errors.append(f"{workflow}:{job} reaches origin with persist-credentials: false")
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
    pattern = "crates/runtime/tests/fixtures/behavior/*planted*"
    nested = "crates/runtime/tests/fixtures/behavior/a/b/planted-key"
    if off == "ignores" or go_glob("**/fixtures/**/planted*") == r"^$":
        errors.append("shared gitleaks ignores are not converted to regular expressions")
    if fnmatch.fnmatch(nested, pattern) != bool(re.match(go_glob(pattern).replace(r"\z", r"\Z"), nested)):
        errors.append("gitleaks and public-surface ignores disagree on a nested planted path")
    if not SUPPRESSION.search("uses: x/y@v1 # zizmor: ignore[unpinned-uses] mirror SHA is unverified"):
        errors.append("a same-line zizmor suppression was not counted")
    original = BASELINE
    with tempfile.TemporaryDirectory() as tmp:
        try:
            BASELINE = pathlib.Path(tmp) / "scan.json"
            seeded = [[["probe", "rule"], 0], [["skipped", "rule"], 7]]
            BASELINE.write_text(json.dumps(seeded) + "\n")
            output = io.StringIO()
            try:
                with contextlib.redirect_stdout(output):
                    save_baseline([("probe", "rule", "planted.txt", 1)], {"probe"}, off)
            except SystemExit as error:
                if error.code != 1 or "refusing to grow the scan baseline" not in output.getvalue():
                    errors.append("baseline --update refused growth for the wrong reason")
            else:
                errors.append("baseline --update grew an existing rule baseline")
            if json.loads(BASELINE.read_text()) != seeded:
                errors.append("baseline --update changed a refused baseline")
            with contextlib.redirect_stdout(io.StringIO()):
                save_baseline([("probe", "rule", "planted.txt", 1)], {"probe"}, "update")
            expected = [[["probe", "rule"], 1], [["skipped", "rule"], 7]]
            if json.loads(BASELINE.read_text()) != expected:
                errors.append("baseline --update changed a scanner that did not run")
            BASELINE.unlink()
            with contextlib.redirect_stdout(io.StringIO()):
                save_baseline([], {"probe"})
            if not BASELINE.is_file():
                errors.append("baseline --update wrote no file")
        finally:
            BASELINE = original
    if "history" in fixtures:
        findings = [] if off == "history" else fixtures["history"]
        if len(findings) != 2:
            errors.append(f"rewritten-history scan returned {len(findings)} findings, expected 2")
    if "diff" in fixtures:
        findings = [] if off == "diff" else fixtures["diff"]
        actual = [(rule, path) for _, rule, path, _ in findings]
        if actual != [("yi-aws-access-key", ".env")]:
            errors.append(f"PR-diff scan returned {actual}, expected only the branch key")
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
        return {"gitleaks-history": lambda: gitleaks("git", config, "gitleaks-history", repo=pathlib.Path.cwd())}
    config = gitleaks_config(True, "gitleaks.toml")
    return {
        "zizmor": lambda: zizmor(sorted((ROOT / ".forgejo/workflows").glob("*.yml"))),
        "gitleaks-tree": lambda: gitleaks("dir", config),
        "gitleaks-diff": lambda: gitleaks("git", config),
        "semgrep": lambda: semgrep(SCAN_ROOTS),
    }


def fixture_tasks():
    config = gitleaks_config(False, "gitleaks-fixtures.toml")
    return {
        "zizmor": lambda: zizmor(sorted((FIXTURES / "zizmor").glob("*.yml"))),
        "gitleaks": lambda: gitleaks("dir", config, repo=FIXTURES / "gitleaks"),
        "semgrep": lambda: semgrep([FIXTURES / "semgrep"], True),
    }


def main(argv):
    parser = argparse.ArgumentParser()
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--update", action="store_true")
    parser.add_argument("--history", action="store_true")
    args = parser.parse_args(argv)
    strict = bool(os.environ.get("CI"))
    if args.selfcheck:
        findings, tools = gather(fixture_tasks(), strict)
        fixtures = {tool: [row for row in findings if row[0] == tool] for tool in tools}
        if "gitleaks" in fixtures:
            fixtures.update(repository_fixtures())
        errors = selfcheck(fixtures=fixtures)
        checks = {rule for tool, (cases, _) in FAMILIES.items() if tool in fixtures
                  for rules in cases.values() for rule in rules}
        checks |= {"baseline", "ignores", "update", "credentials"}
        if "gitleaks" in fixtures:
            checks |= {"history", "diff"}
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
