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
from check_public_surface import rule_lines, unscanned

BASELINE = BASE / "scan.json"
# A scanner that ran and found nothing keeps this row, which is what tells the next --update the
# family is known: without it a family whose rows all fell to zero read as brand new, and its first
# finding seeded the shrink-only baseline with no refusal.
TOOL_ROW = ""
GITLEAKS = (ROOT / "scripts/scan/gitleaks.version").read_text().strip()
ZIZMOR = "zizmor==1.31.1"
SEMGREP = "semgrep==1.182.1"
INSTALL_SECS = 300
GITLEAKS_DIR = pathlib.Path("/usr/local/bin")
FIXTURES = ROOT / "scripts/scan/fixtures"
RULES = ROOT / "scripts/scan/rules"
SCAN_ROOTS = ("evals", "scripts", "python", "crates")
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
    "planted-openai-project.txt": {"yi-openai-project-key"},
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


def on_path(directory):
    entries = [entry for entry in os.environ.get("PATH", "").split(os.pathsep) if entry]
    if str(directory) not in entries:
        os.environ["PATH"] = os.pathsep.join([str(directory), *entries])

def provision_gitleaks(strict, script=None):
    """Incident: the CI lanes installed the pinned gitleaks from a workflow step, so the autofix clone
    ran the lane without it and the CI-strict skip failed every pass (#1733). One gate provisions it."""
    script = script or ROOT / "scripts/scan/install_gitleaks.sh"
    if not strict or not script.is_file():
        return
    try:
        proc = subprocess.run([str(script)], capture_output=True, text=True, check=False,
                              timeout=INSTALL_SECS, env=environment())
    except (OSError, subprocess.TimeoutExpired) as error:
        print(f"scan: the gitleaks install did not finish: {error}")
        return
    if proc.returncode:
        print(f"scan: the gitleaks install failed ({proc.returncode}): {(proc.stdout + proc.stderr).strip()[-400:]}")


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
    translated = fnmatch.translate(pattern).replace("(?>", "(?:")
    translated = re.sub(r"\(\?=\(\?P<g\d+>(.*?)\)\)\(\?P=g\d+\)", r"\1", translated)
    return translated.replace(r"\Z", r"\z")


def tracked():
    proc = subprocess.run(["git", "-C", str(ROOT), "ls-files"], capture_output=True, text=True, check=False)
    return proc.stdout.splitlines() if not proc.returncode else []


def gitleaks_ignores():
    """A shared ignore reaches gitleaks only when every file it matches is planted under a fixtures
    directory, so a path a PR adds for real source is still scanned."""
    paths = tracked()
    return tuple(pattern for pattern in unscanned()
                 if (hits := [path for path in paths if fnmatch.fnmatch(path, pattern)])
                 and all("fixtures" in pathlib.PurePosixPath(path).parts for path in hits))


def gitleaks_config(ignores, name):
    patterns = [go_glob(pattern) for pattern in gitleaks_ignores()] + [r"(^|/)target/.*$"] if ignores else []
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
    counts.update({(tool, TOOL_ROW): 0 for tool in tools if not any(key[0] == tool for key in counts)})
    ordered = sorted([list(key), count] for key, count in counts.items() if count or key[1] == TOOL_ROW)
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
    planted = "scripts/scan/fixtures/gitleaks/planted-aws.txt"
    patterns = () if off == "ignores" else unscanned()
    for pattern in patterns:
        translated = go_glob(pattern)
        if any(token in translated for token in ("(?=", "(?>", "(?P=")) or translated.endswith(r"\Z"):
            errors.append(f"gitleaks ignore is not RE2-compatible: {pattern}")
        if fnmatch.fnmatch(planted, pattern) != bool(re.match(translated.replace(r"\z", r"\Z"), planted)):
            errors.append(f"gitleaks and public-surface ignores disagree on {pattern}")
    if not any(fnmatch.fnmatch(planted, pattern) for pattern in patterns):
        errors.append(f"no shared ignore covers the planted fixture {planted}")
    for real in ("crates/runtime/src/memory/doc.rs", "skills/yi/session-mining/extract.py"):
        if any(fnmatch.fnmatch(real, pattern) for pattern in gitleaks_ignores()):
            errors.append(f"gitleaks skips real source: {real}")
    deny = set(rule_lines(ROOT / "scripts/mirror/deny.txt"))
    for regex in re.findall(r"^regex = '''(.+)'''$", (RULES / "gitleaks.toml").read_text(), re.M):
        if regex not in deny:
            errors.append(f"gitleaks rule {regex!r} is not a credential line in scripts/mirror/deny.txt")
    if not SUPPRESSION.search("uses: x/y@v1 # zizmor: ignore[unpinned-uses] mirror SHA is unverified"):
        errors.append("a same-line zizmor suppression was not counted")
    for strict in (True, False):
        printed, aborted = io.StringIO(), False
        try:
            with contextlib.redirect_stdout(printed):
                unavailable("probe", strict)
        except SystemExit:
            aborted = True
        if aborted != strict:
            errors.append(f"a missing scanner with CI {'set did not abort' if strict else 'unset aborted the run'}")
        if not strict and "skip scan (probe)" not in printed.getvalue():
            errors.append("the announced skip row was not printed")
    with tempfile.TemporaryDirectory() as tmp:
        dest = pathlib.Path(tmp) / "bin"
        dest.mkdir()
        marker = dest / "ran"
        installer = pathlib.Path(tmp) / "install.sh"
        installer.write_text(f"#!/bin/sh\ntouch '{marker}'\n")
        installer.chmod(0o755)
        (dest / "gitleaks").write_text(f"#!/bin/sh\necho {GITLEAKS}\n")
        (dest / "gitleaks").chmod(0o755)
        provision_gitleaks(False, installer)
        if marker.exists():
            errors.append("the scan gate provisioned gitleaks outside CI")
        provision_gitleaks(True, installer if off != "provision" else pathlib.Path(tmp) / "absent.sh")
        if not marker.exists():
            errors.append("a gitleaks at PATH that prints the pinned version kept the installer from running")
        path = os.environ.get("PATH", "")
        try:
            on_path(dest)
            if shutil.which("gitleaks") != str(dest / "gitleaks"):
                errors.append("the gate does not search the directory its installer writes to")
        finally:
            os.environ["PATH"] = path
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
            with contextlib.redirect_stdout(io.StringIO()):
                save_baseline([], {"fresh"})
            expected = [[["fresh", ""], 0], [["probe", "rule"], 1], [["skipped", "rule"], 7]]
            if json.loads(BASELINE.read_text()) != expected:
                errors.append("a scanner that ran with no findings left no row")
            grown = io.StringIO()
            try:
                with contextlib.redirect_stdout(grown):
                    save_baseline([("fresh", "rule", "planted.txt", 1)], {"fresh"}, off)
            except SystemExit:
                refused = "refusing to grow the scan baseline" in grown.getvalue()
            else:
                refused = False
            if off != "baseline" and not refused:
                errors.append("a scanner with a presence row absorbed its first finding")
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
    on_path(GITLEAKS_DIR)
    provision_gitleaks(strict)
    if args.selfcheck:
        findings, tools = gather(fixture_tasks(), strict)
        absent = sorted(set(FAMILIES) - tools)
        if absent:
            print(f"skip scan selfcheck ({', '.join(absent)} did not run, so their fixture families are unproved; "
                  f"the {len(FAMILIES) - len(absent)} of {len(FAMILIES)} families that ran and every scannerless check still do)")
        fixtures = {tool: [row for row in findings if row[0] == tool] for tool in tools}
        if "gitleaks" in fixtures:
            fixtures.update(repository_fixtures())
        errors = selfcheck(fixtures=fixtures)
        checks = {rule for tool, (cases, _) in FAMILIES.items() if tool in fixtures
                  for rules in cases.values() for rule in rules}
        checks |= {"baseline", "ignores", "update", "credentials", "provision"}
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
