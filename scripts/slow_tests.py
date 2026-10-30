#!/usr/bin/env python3
"""Generate and validate the opt-in quick-test filter from nextest JUnit timings."""
import json
import re
import subprocess
import sys
import tempfile
import tomllib
import xml.etree.ElementTree as ET
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
CONFIG = ROOT / ".config/nextest.toml"
BEGIN = "# BEGIN GENERATED QUICK PROFILES"
END = "# END GENERATED QUICK PROFILES"
ROW = re.compile(r"binary_id\(=((?:\\.|[^\\)])*)\) & test\(=((?:\\.|[^\\)])*)\)")


class CheckFailure(Exception):
    pass


def slow_rows(junit):
    rows = set()
    for suite in ET.parse(junit).getroot().iter("testsuite"):
        binary = suite.attrib["name"]
        for case in suite.iter("testcase"):
            if float(case.attrib["time"]) >= 5:
                rows.add((binary, case.attrib["name"]))
    return sorted(rows)


def escape(value):
    replacements = {
        "\\": "\\\\", ")": "\\)", ",": "\\,", "/": "\\/",
        "\n": "\\n", "\r": "\\r", "\t": "\\t",
    }
    return "".join(replacements.get(char, char) for char in value)


def unescape(value):
    replacements = {"n": "\n", "r": "\r", "t": "\t"}
    return re.sub(r"\\(.)", lambda match: replacements.get(match.group(1), match.group(1)), value)


def filterset(rows):
    if not rows:
        return "all()"
    tests = " |\n".join(
        f"  (binary_id(={escape(binary)}) & test(={escape(name)}))" for binary, name in rows
    )
    return f"not (\n{tests}\n)"


def parse_rows(expression):
    return {(unescape(binary), unescape(name)) for binary, name in ROW.findall(expression)}


def write_config(rows, path=CONFIG):
    expression = filterset(rows)
    block = (
        f"{BEGIN}\n"
        "[profile.quick]\ninherits = \"local\"\n"
        f"default-filter = '''\n{expression}\n'''\n\n"
        "[profile.quick-ci]\ninherits = \"default\"\n"
        f"default-filter = '''\n{expression}\n'''\n{END}"
    )
    text = path.read_text()
    if BEGIN in text:
        before, rest = text.split(BEGIN, 1)
        _, after = rest.split(END, 1)
        text = before + block + after
    else:
        text = text.rstrip() + "\n\n" + block + "\n"
    path.write_text(text)


def nextest_list(expression=None):
    command = ["cargo", "nextest", "list", "--workspace", "--message-format", "json"]
    if expression is not None:
        command += ["-E", expression]
    run = subprocess.run(command, cwd=ROOT, capture_output=True, text=True)
    if run.returncode:
        raise CheckFailure(f"`{' '.join(command[:4])}` exited {run.returncode}: {run.stderr.strip()}")
    try:
        document = json.loads(run.stdout)
    except ValueError as error:
        raise CheckFailure(f"nextest list returned invalid JSON: {error}") from error
    return {
        (suite["binary-id"], name)
        for suite in document["rust-suites"].values()
        for name, case in suite["testcases"].items()
        if case["filter-match"]["status"] == "matches"
    }


def validate_recipe(text):
    if re.search(r"\bquick(?:-ci)?\b|default-filter|--filterset|\s-E\s", text):
        raise CheckFailure("just test selects the quick filterset")


def crate_dir(package, root=ROOT):
    for manifest in sorted((root / "crates").glob("*/Cargo.toml")):
        if re.search(rf'^name = "{re.escape(package)}"$', manifest.read_text(), re.M):
            return manifest.parent
    return None


def defined_in_source(binary, name, root=ROOT):
    """A row missing here is explainable only by a cfg gate on that very function: a name that
    survives as an ungated helper, or in another crate, still reads as stale."""
    crate = crate_dir(binary.split("::", 1)[0], root)
    if crate is None:
        return False
    leaf = name.rsplit("::", 1)[-1]
    found = subprocess.run(
        ["git", "grep", "-n", "-F", f"fn {leaf}(", "--", crate.relative_to(root).as_posix()],
        cwd=root, capture_output=True, text=True,
    )
    for hit in found.stdout.splitlines():
        path, _, location = hit.partition(":")
        lines = (root / path).read_text().splitlines()
        line = int(location.split(":", 1)[0])
        if cfg_gated(lines, line - 1):
            return True
    return False


def cfg_gated(lines, index):
    for context in lines:
        stripped = context.strip()
        if stripped.startswith("#![cfg("):
            return True
        if not (stripped.startswith("#!") or stripped.startswith("//") or not stripped):
            break
    for context in reversed(lines[:index]):
        stripped = context.strip()
        if stripped.startswith("#[cfg("):
            return True
        if stripped.startswith("#[") or stripped.startswith("//") or not stripped:
            continue
        return False
    return False


def validate(rows, all_tests, filtered_tests, recipe, defined=defined_in_source):
    absent = rows - all_tests
    gated = {row for row in absent if defined(*row)}
    errors = [f"stale slow test: {binary} {name}" for binary, name in sorted(absent - gated)]
    excluded = all_tests - filtered_tests
    errors += [f"slow filter failed to exclude: {binary} {name}" for binary, name in sorted((rows - gated) - excluded)]
    errors += [f"slow filter excludes unlisted test: {binary} {name}" for binary, name in sorted(excluded - rows)]
    try:
        validate_recipe(recipe)
    except CheckFailure as error:
        errors.append(str(error))
    if errors:
        raise CheckFailure("\n".join(errors))
    if gated:
        print(f"quick: {len(gated)} listed tests are not compiled on this platform: " + ", ".join(sorted(name for _, name in gated)))


def config_rows():
    with CONFIG.open("rb") as stream:
        profiles = tomllib.load(stream)["profile"]
    expression = profiles["quick"]["default-filter"]
    if profiles["quick"].get("inherits") != "local" or profiles["quick-ci"].get("inherits") != "default":
        raise CheckFailure("quick profiles must inherit local outside CI and default under CI")
    if profiles["quick-ci"]["default-filter"] != expression:
        raise CheckFailure("quick and quick-ci slow filtersets differ")
    return expression, parse_rows(expression)


def dry_run(recipe):
    run = subprocess.run(["just", "--dry-run", recipe], cwd=ROOT, capture_output=True, text=True)
    if run.returncode:
        raise CheckFailure(f"just --dry-run {recipe} exited {run.returncode}: {run.stderr.strip()}")
    return run.stdout


def repo_check():
    expression, rows = config_rows()
    all_tests = nextest_list()
    filtered_tests = nextest_list(expression)
    validate(rows, all_tests, filtered_tests, dry_run("test"))
    print(
        f"quick: {len(rows)} tests skipped as slow (>=5 s, list .config/nextest.toml, "
        "regenerate: just slow-tests)"
    )


def expect_failure(name, function, needle):
    try:
        function()
    except CheckFailure as error:
        if needle not in str(error):
            raise CheckFailure(f"selfcheck {name} failed for the wrong reason: {error}") from error
        print(f"seen red {name}: {error}")
        return
    raise CheckFailure(f"selfcheck {name} did not fail")


def selfcheck_generator():
    with tempfile.TemporaryDirectory() as directory:
        directory = Path(directory)
        junit = directory / "timing.xml"
        junit.write_text("""<testsuites>
<testsuite name="yi-a"><testcase name="mod::same" time="5.000"/><testcase name="mod::fast" time="4.999"/></testsuite>
<testsuite name="yi-b"><testcase name="mod::same" time="6.000"/></testsuite>
</testsuites>""")
        rows = slow_rows(junit)
        assert rows == [("yi-a", "mod::same"), ("yi-b", "mod::same")], rows
        assert filterset([]) == "all()"
        config = directory / "nextest.toml"
        config.write_text("[profile.default]\n")
        write_config(rows, config)
        write_config(rows, config)
        with config.open("rb") as stream:
            profiles = tomllib.load(stream)["profile"]
        assert parse_rows(profiles["quick"]["default-filter"]) == set(rows)
        assert config.read_text().count(BEGIN) == 1
        escaped = [("yi-a", "mod::slash/comma,)")]
        assert parse_rows(filterset(escaped)) == set(escaped)


def selfcheck_predicate():
    with tempfile.TemporaryDirectory() as directory:
        root = Path(directory)
        for crate, name, body in (
            ("demo", "yi-demo", '#[cfg(target_os = "macos")]\nfn gated_here() {}\nfn ungated_here() {}\n'),
            ("other", "yi-other", '#[cfg(target_os = "macos")]\nfn only_elsewhere() {}\n'),
        ):
            (root / "crates" / crate).mkdir(parents=True)
            (root / "crates" / crate / "Cargo.toml").write_text(f'[package]\nname = "{name}"\n')
            (root / "crates" / crate / "lib.rs").write_text(body)
        (root / "crates/demo/sandbox.rs").write_text(
            '#![cfg(target_os = "macos")]\n\nfn helper() {\n}\n\n#[test]\nfn gated_file() {}\n'
        )
        subprocess.run(["git", "init", "-q"], cwd=root, check=True)
        subprocess.run(["git", "add", "crates"], cwd=root, check=True)
        assert defined_in_source("yi-demo::integration", "mod::gated_here", root)
        assert defined_in_source("yi-demo::integration", "sandbox::gated_file", root), "a file-level cfg gate read as stale"
        assert not defined_in_source("yi-demo::integration", "mod::ungated_here", root), "an ungated helper excused a stale row"
        assert not defined_in_source("yi-demo::integration", "mod::only_elsewhere", root), "another crate's function excused a stale row"


def selfcheck():
    selfcheck_generator()
    selfcheck_predicate()
    _, rows = config_rows()
    recipe = dry_run("test")
    validate_recipe(recipe)
    known = {("yi-x::integration", "mod::a"), ("yi-x::integration", "mod::b")}
    validate(known, known, set(), recipe)
    expect_failure(
        "a row this platform does not run",
        lambda: validate(known | {("yi-x::integration", "mod::gone")}, known, set(), recipe),
        "stale slow test: yi-x::integration mod::gone",
    )
    expect_failure(
        "filter membership",
        lambda: validate(known, known, {("yi-x::integration", "mod::a")}, recipe),
        "slow filter failed to exclude: yi-x::integration mod::a",
    )
    expect_failure(
        "filter excludes unlisted test",
        lambda: validate({("yi-x::integration", "mod::a")}, known, set(), recipe),
        "slow filter excludes unlisted test: yi-x::integration mod::b",
    )
    expect_failure(
        "full recipe",
        lambda: validate_recipe("cargo nextest run --workspace --profile quick"),
        "just test selects the quick filterset",
    )
    print(f"ok   slow_tests selfcheck ({len(rows)} listed rows)")


def main(argv):
    if argv == ["--selfcheck"]:
        selfcheck()
        return 0
    if argv == ["--check"]:
        repo_check()
        return 0
    if len(argv) != 1:
        sys.exit("usage: slow_tests.py JUNIT | --check | --selfcheck")
    rows = slow_rows(argv[0])
    write_config(rows, CONFIG)
    print(f"slow-tests: wrote {len(rows)} tests at >=5 s to {CONFIG}")
    return 0


if __name__ == "__main__":
    try:
        sys.exit(main(sys.argv[1:]))
    except (CheckFailure, KeyError, ValueError, ET.ParseError) as error:
        sys.exit(f"slow-tests: {error}")
