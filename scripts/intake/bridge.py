#!/usr/bin/env python3
import argparse
import dataclasses
import functools
import hashlib
import json
import os
import pathlib
import re
import subprocess
import sys
import tempfile
import time
import urllib.request
from typing import Any, Callable, Optional

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "scripts"))
sys.path.insert(0, str(ROOT / "scripts/guardrails"))
import check_public_surface as surface
import forge_pr
import pr_review

ALLOW = "ALLOW"
GITHUB_REPO = "jackmaxwil/yi"
FORGE_REPO = "apex/yi"
LENS_SCHEMA = {"type": "object", "required": ["refusals"], "properties": {"refusals": {"type": "array", "items": {"type": "object", "required": ["path", "line", "reason"], "properties": {"path": {"type": "string"}, "line": {"type": "integer"}, "reason": {"type": "string"}}}}}}
OVERRIDABLE = {"commits", "files", "lines", "bytes", "network-literal", "unsafe", "lens"}
CAPS = ("commits", "files", "lines", "bytes")


@dataclasses.dataclass(frozen=True)
class Refusal:
    code: str
    text: str


@dataclasses.dataclass(frozen=True)
class AddedLine:
    path: str
    line: int
    text: str


@dataclasses.dataclass
class Change:
    path: str
    old_path: Optional[str] = None
    old_mode: Optional[str] = "100644"
    new_mode: Optional[str] = "100644"
    binary: bool = False
    rename: bool = False
    added: list[AddedLine] = dataclasses.field(default_factory=list)
    deleted: int = 0


@dataclasses.dataclass
class Commit:
    sha: str
    author: str
    committer: str
    parents: int = 1
    signature: bool = True


@dataclasses.dataclass
class Case:
    number: int
    login: str
    state: str
    base_repo: str
    base_ref: str
    head: str
    fetched_head: str
    current_head: str
    merge_base: str
    forge_base: str
    current_main: str
    commits: list[Commit]
    changes: list[Change]
    patch: bytes
    contributor: dict[str, Any]
    mapped_bases: tuple[str, ...] = ("mapped",)
    base_is_ancestor: bool = True
    apply_check: Callable[[bytes, str], Optional[str]] = lambda _patch, _base: None
    lens: Callable[[str], Any] = lambda _prompt: {"refusals": []}
    user_id: Optional[int] = None


@dataclasses.dataclass
class Outcome:
    decision: Any
    checks: list[dict[str, str]]
    overridden: Optional[Refusal] = None


@dataclasses.dataclass
class ImportResult:
    decision: Any
    branch: Optional[str] = None
    draft: Optional[dict[str, Any]] = None
    audit: Optional[dict[str, Any]] = None


def refusal(code: str, text: str) -> Refusal:
    return Refusal(code, f"intake: {code}: {text}")


def added_lines(case: Case) -> list[AddedLine]:
    return [line for change in case.changes for line in change.added]


def paths(case: Case) -> set[str]:
    return {p for change in case.changes for p in (change.old_path, change.path) if p and p != "/dev/null"}


def check_01_allowlist(case: Case) -> Any:
    contributors = load_allowed()["contributors"]
    if case.login not in contributors:
        return refusal("allowlist", f"GitHub login '{case.login}' is not allowed")
    expected_id = contributors[case.login].get("user_id")
    if expected_id is not None and case.user_id != expected_id:
        return refusal("allowlist", f"GitHub login '{case.login}' is not allowed")
    if case.state != "open" or (case.base_repo, case.base_ref) != (GITHUB_REPO, "main"):
        actual = f"{case.base_repo}:{case.base_ref}"
        return refusal("base", f"PR #{case.number} targets {actual}, expected {GITHUB_REPO}:main")
    return ALLOW


def check_02_signatures(case: Case) -> Any:
    for commit in case.commits:
        if not commit.signature:
            return refusal("signature", f"commit {commit.sha} has no signature verifiable by {case.login}'s pinned keys")
    return ALLOW


def check_03_identities(case: Case) -> Any:
    emails = set(case.contributor["emails"])
    for commit in case.commits:
        if commit.author not in emails or commit.committer not in emails:
            return refusal("identity", f"commit {commit.sha} author '{commit.author}' or committer '{commit.committer}' is not pinned to {case.login}")
    return ALLOW


def check_04_head(case: Case) -> Any:
    if case.fetched_head != case.head:
        return refusal("head", f"fetched {case.fetched_head}, expected pinned {case.head}")
    if case.current_head != case.head:
        return refusal("head", f"PR #{case.number} moved from {case.head} to {case.current_head}; no branch pushed")
    return ALLOW


def check_05_commits(case: Case) -> Any:
    cap = case.contributor["caps"]["commits"]
    count = len(case.commits)
    if count > cap:
        return refusal("commits", f"{count} exceeds per-login cap {cap}")
    return ALLOW


def check_06_files(case: Case) -> Any:
    cap = case.contributor["caps"]["files"]
    count = len(paths(case))
    if count > cap:
        return refusal("files", f"{count} exceeds per-login cap {cap}")
    return ALLOW


def check_07_lines(case: Case) -> Any:
    cap = case.contributor["caps"]["lines"]
    count = len(added_lines(case)) + sum(change.deleted for change in case.changes)
    if count > cap:
        return refusal("lines", f"{count} exceeds per-login cap {cap}")
    return ALLOW


def check_08_bytes(case: Case) -> Any:
    cap = case.contributor["caps"]["bytes"]
    count = len(case.patch)
    if count > cap:
        return refusal("bytes", f"{count} exceeds per-login cap {cap}")
    return ALLOW


def check_09_base_history(case: Case) -> Any:
    if len(case.mapped_bases) != 1 or not case.base_is_ancestor:
        return refusal("base", f"mirror merge-base {case.merge_base} has no unique Forgejo mapping")
    for commit in case.commits:
        if commit.parents != 1:
            return refusal("history", f"merge commit {commit.sha} is not importable; rebase into linear signed commits")
    return ALLOW


@functools.cache
def excluded_paths() -> list[str]:
    return surface.rule_lines(surface.RULES / "exclude.txt")


def is_build_rs(path: str) -> bool:
    return path.lower().rsplit("/", 1)[-1] == "build.rs"


def deny_rule(path: str) -> Optional[str]:
    """Incident class: the owner builds on a case-insensitive disk, so .Cargo/ and BUILD.RS are the denied names."""
    path = path.lower()
    if is_build_rs(path):
        return "build.rs"
    # Cargo reads .cargo/config* from the working directory and every parent; a rustc-wrapper,
    # linker or source there runs in every later build.
    if ".cargo" in path.split("/"):
        return ".cargo"
    if pr_review.walled([path]):
        return next(rule for rule in pr_review.WALL if path.startswith(rule))
    extra = ("scripts/mirror/", "scripts/intake/", "deny.toml", "rust-toolchain.toml", "allowed_signers", ".gitguardian.yaml")
    rule = next((rule for rule in extra if path == rule.rstrip("/") or path.startswith(rule)), None)
    if rule:
        return rule
    excluded = next((rule for rule in excluded_paths() if surface.excluded(path, [rule.lower()])), None)
    if excluded:
        return f"mirror exclude.txt: {excluded}"
    return None


def check_10_deny_paths(case: Case) -> Any:
    for change in case.changes:
        for path in (change.old_path, change.path):
            if path and path != "/dev/null":
                rule = deny_rule(path)
                if rule:
                    return refusal("path", f"'{path}' is denied by {rule}")
    return ALLOW


def check_11_symlinks(case: Case) -> Any:
    for change in case.changes:
        if "120000" in (change.old_mode, change.new_mode):
            return refusal("mode", f"'{change.path}' is a symlink")
    return ALLOW


def check_12_submodules(case: Case) -> Any:
    for change in case.changes:
        if "160000" in (change.old_mode, change.new_mode):
            return refusal("mode", f"'{change.path}' is a submodule")
    return ALLOW


def check_13_executables(case: Case) -> Any:
    for change in case.changes:
        if change.old_mode != change.new_mode and "100755" in (change.old_mode, change.new_mode):
            return refusal("mode", f"'{change.path}' changes the executable bit")
    return ALLOW


def check_14_binary(case: Case) -> Any:
    for change in case.changes:
        if change.binary:
            return refusal("binary", f"'{change.path}' is binary")
    return ALLOW


def check_15_lfs(case: Case) -> Any:
    for line in added_lines(case):
        if line.text.startswith("version https://git-lfs.github.com/spec/v1"):
            return refusal("lfs", f"'{line.path}' is an LFS pointer")
    return ALLOW


def escaped(path: str) -> str:
    return path.encode("unicode_escape", "backslashreplace").decode("ascii").replace("'", "\\'")


def filename_problem(path: str) -> Optional[str]:
    try:
        path.encode("utf-8")
    except UnicodeEncodeError:
        return "invalid UTF-8"
    if path.startswith("/") or re.match(r"^[A-Za-z]:", path):
        return "absolute path"
    if "\\" in path:
        return "backslash"
    parts = path.split("/")
    if any(part in ("", ".", "..") for part in parts):
        return "empty, '.' or '..' component"
    if ".git" in parts:
        return ".git component"
    if any(ord(char) < 32 or ord(char) == 127 for char in path):
        return "control character"
    if not path.isascii():
        return "non-ASCII/confusable"
    return None


def check_16_filenames(case: Case) -> Any:
    all_paths = sorted(paths(case))
    folded: dict[str, str] = {}
    for path in all_paths:
        problem = filename_problem(path)
        if problem:
            return refusal("filename", f"'{escaped(path)}' fails {problem}")
        key = path.casefold()
        if key in folded and folded[key] != path:
            return refusal("filename", f"'{escaped(path)}' fails case-fold collision with '{escaped(folded[key])}'")
        folded[key] = path
    return ALLOW


@functools.cache
def deny_patterns() -> list[re.Pattern[str]]:
    return [re.compile(rule) for rule in surface.rule_lines(surface.RULES / "deny.txt")]


def check_17_secrets(case: Case) -> Any:
    for line in added_lines(case):
        if any(pattern.search(line.text) for pattern in deny_patterns()):
            return refusal("content", f"'{line.path}:{line.line}' matches a secret/private pattern")
    return ALLOW


@functools.cache
def replaced_literals() -> list[str]:
    return [old for old, _ in surface.swaps_of(surface.rule_lines(surface.RULES / "replace.txt", comments=False))]


def check_18_markers(case: Case) -> Any:
    for line in added_lines(case):
        if "***REMOVED***" in line.text or any(old in line.text for old in replaced_literals()):
            return refusal("content", f"'{line.path}:{line.line}' contains a mirror replacement literal")
    return ALLOW


# Bare text is a hostname only on a common TLD, and not when a call follows (`self.app.run()`).
# Where syntax makes a name an endpoint (host:port, user@host, an scp or rsync `host:` destination,
# the first argument of a network command) any TLD counts; a bare rarer one, even quoted, reads like
# `row.id` or `"response.failed"`. In host:port a file extension is a file (`main.rs:42`), except
# that rs, md, py and sh, which are also TLDs, count on a well-known port.
LABELS = r"(?:[a-z0-9-]+\.)+"
COMMON = "com|net|org|edu|gov|mil|io|dev|app|cloud|lan|local|internal|invalid|onion|arpa"
FILE_EXT = "toml|jsonl?|ya?ml|txt|lock|js|ts|tsx|jsx|mjs|cjs|cfg|ini|conf|log|html?|css|csv|xml|svg|sql|go|zig|cpp|hpp|cxx|hxx|rb|java|kt|cs|php|swift|proto|lua|exs?|rst|tex|png|jpe?g|gif|pdf|webp|ico|wasm"
FILE_TLD = "rs|md|py|sh"
WELL_KNOWN = "22|25|53|80|110|143|443|465|587|993|995|1080|3128|8000|8080|8443|8888|9000"
ANY_HOST = rf"{LABELS}[a-z]{{2,}}\b"
FILE_FREE_HOST = rf"{LABELS}(?!(?:{FILE_EXT})\b)[a-z]{{2,}}\b"
HOSTNAME = re.compile(
    rf"(?i)\b{LABELS}(?:{COMMON})\b(?!\(|\.\w+\()"
    rf"|(?<![\w.]){LABELS}(?!(?:{FILE_EXT}|{FILE_TLD})\b)[a-z]{{2,}}:\d{{1,5}}\b"
    rf"|(?<![\w.])(?<![^/]/){LABELS}(?:{FILE_TLD}):(?:{WELL_KNOWN})\b"
    rf"|(?<=\w)@{ANY_HOST}"
    rf"|\b(?:scp|rsync)\b[^\n]*?\s{FILE_FREE_HOST}::?"
    rf"|\b(?:curl|wget|ssh|sftp|nc|ncat|telnet|ping|dig|nslookup)\b(?:\s+-\S+(?:\s+(?:\d+|\"[^\"]*\"|'[^']*'))?)*\s+{ANY_HOST}"
)
# The keyword may sit alone before its brace on the next line, so a prose line ending in it is refused too.
UNSAFE = re.compile(r"\bunsafe\s*(?:\{|//|/\*|$)|\bunsafe\s+(?:fn|impl|trait|extern)\b|(?:#!?\[|cfg_attr\().*\bunsafe\s*\(|^\s*unsafe\s*\(")


def network_literal(text: str) -> Optional[str]:
    if re.search(r"(?i)\b[a-z][a-z0-9+.-]*://", text):
        return "URL"
    if re.search(r"\b(?:\d{1,3}\.){3}\d{1,3}\b", text) or re.search(r"(?i)\b(?:[0-9a-f]{1,4}:){2,7}[0-9a-f]{0,4}\b", text):
        return "IP literal"
    if re.search(r"(?i)\blocalhost\b", text) or HOSTNAME.search(text):
        return "hostname"
    return None


def check_19_network(case: Case) -> Any:
    for line in added_lines(case):
        kind = network_literal(line.text)
        if kind:
            return refusal("network-literal", f"'{line.path}:{line.line}' contains {kind}; owner review is required")
    return ALLOW


def check_20_unsafe(case: Case) -> Any:
    for line in added_lines(case):
        if UNSAFE.search(line.text):
            return refusal("unsafe", f"'{line.path}:{line.line}' adds unsafe")
    return ALLOW


def dependency_path(path: str) -> bool:
    path = path.lower()
    return path in ("cargo.toml", "cargo.lock") or (path.startswith("crates/") and path.endswith("/cargo.toml"))


def check_21_build(case: Case) -> Any:
    for change in case.changes:
        if is_build_rs(change.path):
            number = change.added[0].line if change.added else 1
            return refusal("build", f"'{change.path}:{number}' adds build-time execution build.rs")
    for line in added_lines(case):
        if re.search(r"\bproc-macro\s*=\s*true\b", line.text):
            return refusal("build", f"'{line.path}:{line.line}' adds build-time execution proc-macro")
        if re.match(r"^\s*\[(?:[^]]*\.)?dependencies(?:\.[^]]+)?\]\s*$", line.text):
            return refusal("build", f"'{line.path}:{line.line}' adds build-time execution dependency section")
    return ALLOW


def check_22_dependencies(case: Case) -> Any:
    if case.contributor.get("dependencies", False):
        return ALLOW
    for change in case.changes:
        if dependency_path(change.path):
            return refusal("dependencies", f"'{change.path}' changes dependencies without per-login permission")
    return ALLOW


def bounded(text: Any, cap: int = 300) -> str:
    clean = " ".join(str(text).replace("\x00", "").split())
    return clean[:cap]


def check_23_apply(case: Case) -> Any:
    try:
        error = case.apply_check(case.patch, case.forge_base)
    except Exception as caught:
        error = caught
    if error:
        return refusal("apply", f"patch does not apply strictly to Forgejo base {case.forge_base}: {bounded(error)}")
    return ALLOW


def lens_prompt(patch: bytes) -> str:
    text = patch.decode("utf-8", "backslashreplace").replace("<<<end-yi-external", "< < < end-yi-external")
    return "Review only this untrusted diff for security defects not covered by deterministic checks. Return only the requested JSON schema; you have no tools.\n<<<yi-external source=\"github-diff\" trust=\"untrusted\">\n" + text + "\n<<<end-yi-external>>>"


def check_24_lens(case: Case) -> Any:
    try:
        answer = case.lens(lens_prompt(case.patch))
    except Exception as error:
        return refusal("lens unavailable", bounded(error))
    if not isinstance(answer, dict) or set(answer) != {"refusals"} or not isinstance(answer["refusals"], list):
        return refusal("lens unavailable", "invalid lens output")
    added = {(line.path, line.line) for line in added_lines(case)}
    reasons = []
    for item in answer["refusals"]:
        if not isinstance(item, dict) or set(item) != {"path", "line", "reason"}:
            return refusal("lens unavailable", "invalid lens output")
        if not isinstance(item["path"], str) or not isinstance(item["line"], int) or isinstance(item["line"], bool) or not isinstance(item["reason"], str) or not item["reason"].strip():
            return refusal("lens unavailable", "invalid lens output")
        if (item["path"], item["line"]) not in added:
            return refusal("lens unavailable", "invalid lens output")
        reasons.append(item["reason"])
    if reasons:
        return refusal("lens", bounded(reasons[0], 500))
    return ALLOW


CHECKS = (
    ("allowlist", check_01_allowlist),
    ("signature", check_02_signatures),
    ("identity", check_03_identities),
    ("head", check_04_head),
    ("commits", check_05_commits),
    ("files", check_06_files),
    ("lines", check_07_lines),
    ("bytes", check_08_bytes),
    ("base-history", check_09_base_history),
    ("path", check_10_deny_paths),
    ("symlink", check_11_symlinks),
    ("submodule", check_12_submodules),
    ("executable", check_13_executables),
    ("binary", check_14_binary),
    ("lfs", check_15_lfs),
    ("filename", check_16_filenames),
    ("secret", check_17_secrets),
    ("marker", check_18_markers),
    ("network-literal", check_19_network),
    ("unsafe", check_20_unsafe),
    ("build", check_21_build),
    ("dependencies", check_22_dependencies),
    ("apply", check_23_apply),
    ("lens", check_24_lens),
)


def run_checks(case: Case, disabled: frozenset[str] = frozenset(), override: Optional[str] = None) -> Outcome:
    checks = []
    overridden = None
    for name, check in CHECKS:
        if name in disabled:
            checks.append({"check": name, "decision": "DISABLED"})
            continue
        decision = check(case)
        if decision == ALLOW:
            checks.append({"check": name, "decision": ALLOW})
            continue
        if decision.code == override and decision.code in OVERRIDABLE:
            overridden = decision
            checks.append({"check": name, "decision": f"OVERRIDE {decision.text}"})
            continue
        checks.append({"check": name, "decision": decision.text})
        return Outcome(decision, checks, overridden)
    return Outcome(ALLOW, checks, overridden)


def evaluate(case: Case, disabled: frozenset[str] = frozenset(), override: Optional[str] = None) -> Any:
    return run_checks(case, disabled, override).decision


def load_allowed() -> dict[str, Any]:
    return json.loads((ROOT / "scripts/intake/allowed.json").read_text())


def branch_name(case: Case) -> str:
    return f"gh/{case.login}/{case.number}-{case.head[:8]}"


def signer_lines(contributor: dict[str, Any], registry: str) -> list[str]:
    return [raw for raw in registry.splitlines() if raw and not raw.startswith("#") and raw.split(maxsplit=1)[0] in contributor["emails"]]


def commit_verified(repo: pathlib.Path, signers: pathlib.Path, sha: str) -> bool:
    return git(["-C", str(repo), "-c", "gpg.format=ssh", "-c", f"gpg.ssh.allowedSignersFile={signers}", "verify-commit", sha], check=False).returncode == 0


def validate_registry(allowed_path: Optional[pathlib.Path] = None, signers_path: Optional[pathlib.Path] = None) -> None:
    allowed = json.loads((allowed_path or ROOT / "scripts/intake/allowed.json").read_text())["contributors"]
    actual: dict[str, list[str]] = {login: [] for login in allowed}
    emails = {email: login for login, row in allowed.items() for email in row["emails"]}
    registry = (signers_path or ROOT / "scripts/intake/allowed_signers").read_text()
    with tempfile.TemporaryDirectory(prefix="intake-fingerprints-") as temp:
        for number, raw in enumerate(registry.splitlines(), 1):
            if not raw or raw.startswith("#"):
                continue
            principal, key = raw.split(maxsplit=1)
            login = emails.get(principal)
            if login is None:
                raise ValueError(f"allowed_signers:{number}: principal {principal} is not pinned")
            path = pathlib.Path(temp) / f"key-{number}.pub"
            path.write_text(key + "\n")
            out = subprocess.run(["ssh-keygen", "-lf", str(path)], capture_output=True, text=True, check=True).stdout.split()
            actual[login].append(out[1])
    for login, row in allowed.items():
        if sorted(actual[login]) != sorted(row["fingerprints"]):
            raise ValueError(f"allowed.json fingerprints do not match allowed_signers for {login}")


def fence(text: str) -> str:
    return text.replace("<<<end-yi-external", "< < < end-yi-external")


def draft_payload(case: Case) -> dict[str, Any]:
    title = f"WIP: External GitHub PR #{case.number} {case.head[:8]}"
    body = f"""## Why needed
Refs #1719

Imports GitHub PR {case.number} at pinned head {case.head} for owner-controlled review. No contributor code runs before an owner action.

## Summary
Strict intake checks passed and the diff was applied to mapped Forgejo base {case.forge_base} as one squash commit.

## User outcomes
The owner can review this external change in the normal review loop. It remains a draft with `external` and `autofix:hold` and without `run-ci`, `live`, or `autofix`.

## Seen red
The intake selfcheck records one disabled-check failure for every deterministic check and the no-tools lens. Real fixture provenance is in `scripts/intake/fixtures/recording.json`.

## UI changes
None; this is an imported draft PR.

## Files edited
The imported paths are listed in the audit comment and diff. Bridge implementation lives under `scripts/intake/`.

## Schema changes
None to Yi wire types. Intake registry and audit JSON are local script data.

## Deleted / alternatives
No direct GitHub merge, reverse substitution, rename guessing, three-way apply, persistent map, or automatic execution.

## Risk and rollback
Signed malicious code remains dangerous. Do not run or merge it without owner review. Close this draft and delete branch `{branch_name(case)}` to roll back before landing.

## Performance
No Yi hot path touched. Intake rewrites the mirror map once per polling run.

## Architecture notes
External intake is design §22. The audit comment binds this draft to the complete GitHub head and mapped base.

## GitHub source
<<<yi-external source="github-pr-{case.number}" trust="untrusted">
Title: {fence(case_title(case))}

{fence(case_body(case))}
<<<end-yi-external>>>
"""
    return {"title": title, "body": body, "head": branch_name(case), "base": "main", "draft": True}


def case_title(case: Case) -> str:
    return getattr(case, "title", f"GitHub PR #{case.number}")


def case_body(case: Case) -> str:
    return getattr(case, "body", "")


def rule_hashes() -> dict[str, str]:
    paths = ["scripts/intake/bridge.py", "scripts/intake/allowed.json", "scripts/intake/allowed_signers", "scripts/mirror/exclude.txt", "scripts/mirror/replace.txt", "scripts/mirror/deny.txt"]
    return {path: hashlib.sha256((ROOT / path).read_bytes()).hexdigest() for path in paths}


def audit_payload(case: Case, imported: str, outcome: Outcome, actor: str, override: Optional[dict[str, str]]) -> dict[str, Any]:
    return {"github_pr": case.number, "github_login": case.login, "head": case.head, "mirror_merge_base": case.merge_base, "forge_base": case.forge_base, "imported": imported, "branch": branch_name(case), "rule_hashes": rule_hashes(), "checks": outcome.checks, "actor": actor, "override": override, "original_refusal": outcome.overridden.text if outcome.overridden else None}


def import_case(case: Case, transport: Any, override: Optional[dict[str, str]] = None) -> ImportResult:
    code = override.get("check") if override else None
    if code and code not in OVERRIDABLE:
        return ImportResult(refusal("override", f"code '{code}' is not {'override' + 'able'}"))
    if override and (transport.actor() != "jack" or override.get("sha") != case.head or not override.get("reason")):
        return ImportResult(Refusal("override", "intake: override: owner actor, full --sha, --check and --reason are required"))
    outcome = run_checks(case, override=code)
    if outcome.decision != ALLOW:
        transport.github("POST", f"repos/{GITHUB_REPO}/issues/{case.number}/comments", {"body": f"{outcome.decision.text}\n\nHead: {case.head}\n\nPush a corrected head; every check will rerun"})
        return ImportResult(outcome.decision)
    imported = transport.apply_and_commit(case)
    current = transport.github("GET", f"repos/{GITHUB_REPO}/pulls/{case.number}")
    moved = current.get("head", {}).get("sha")
    if moved != case.head:
        return ImportResult(refusal("head", f"PR #{case.number} moved from {case.head} to {moved}; no branch pushed"))
    if transport.main_sha() != case.current_main or not transport.workflow_equal(case.current_main, imported):
        return ImportResult(Refusal("base", "intake: base: current main differs in .forgejo/; import stopped before push"))
    branch = branch_name(case)
    existing = transport.branch_sha(branch)
    if existing and existing != imported:
        return ImportResult(refusal("branch", f"{branch} exists at a different commit"))
    if not existing:
        transport.push(imported, branch)
    draft = draft_payload(case)
    created = transport.forge("POST", f"repos/{FORGE_REPO}/pulls", draft)
    number = created["number"]
    transport.forge("POST", f"repos/{FORGE_REPO}/issues/{number}/labels", {"labels": ["external", "autofix:hold"]})
    audit = audit_payload(case, imported, outcome, transport.actor(), override)
    comment = "<!-- yi-intake -->\n```json\n" + json.dumps(audit, indent=2, sort_keys=True) + "\n```"
    transport.forge("POST", f"repos/{FORGE_REPO}/issues/{number}/comments", {"body": comment})
    transport.forge("POST", f"repos/{FORGE_REPO}/issues/{number}/comments", {"body": f"GitHub source: {GITHUB_REPO}#{case.number}"})
    transport.github("POST", f"repos/{GITHUB_REPO}/issues/{case.number}/comments", {"body": f"Imported for owner review as {FORGE_REPO}#{number}; no CI runs before owner approval.\n\n{comment}"})
    transport.supersede(case, number)
    return ImportResult(ALLOW, branch, draft, audit)


def parse_diff_path(raw: str) -> str:
    raw = raw.strip()
    if raw == "/dev/null":
        return raw
    if raw.startswith('"'):
        try:
            raw = json.loads(raw)
        except json.JSONDecodeError:
            return raw
    if raw.startswith(("a/", "b/")):
        return raw[2:]
    return raw


HUNK = re.compile(r"@@ -\d+(?:,(\d+))? \+(\d+)(?:,(\d+))? @@")


def parse_diff(patch: bytes) -> list[Change]:
    """Invariant: hunk counts decide what is content, so a body line that reads as a header never moves a path.
    Split on \n alone, as git does: str.splitlines also breaks on \f and \x1c."""
    changes: list[Change] = []
    current: Optional[Change] = None
    new_line = old_left = new_left = 0
    for raw in patch.decode("utf-8", "surrogateescape").split("\n"):
        if current and (old_left or new_left):
            if raw.startswith("+"):
                current.added.append(AddedLine(current.path or current.old_path or "", new_line, raw[1:]))
                new_line, new_left = new_line + 1, new_left - 1
            elif raw.startswith("-"):
                current.deleted, old_left = current.deleted + 1, old_left - 1
            elif raw == "" or raw.startswith(" "):
                new_line, old_left, new_left = new_line + 1, old_left - 1, new_left - 1
        elif raw.startswith("diff --git "):
            header = raw[len("diff --git "):]
            quoted = re.fullmatch(r'("(?:\\.|[^"])+")\s+("(?:\\.|[^"])+")', header)
            old = new = header[2:(len(header) - 1) // 2]
            if quoted:
                old, new = (parse_diff_path(part) for part in quoted.groups())
            elif header != f"a/{old} b/{new}":
                raise ValueError(f"unparsable diff header: {header[:80]!r}")
            current = Change(new, old)
            changes.append(current)
        elif current is None:
            continue
        elif raw.startswith("old mode "):
            current.old_mode = raw.split()[-1]
        elif raw.startswith("new mode "):
            current.new_mode = raw.split()[-1]
        elif raw.startswith("new file mode "):
            current.old_mode, current.new_mode = None, raw.split()[-1]
        elif raw.startswith("deleted file mode "):
            current.old_mode, current.new_mode = raw.split()[-1], None
        elif raw.startswith("rename from "):
            current.old_path, current.rename = raw[len("rename from "):], True
        elif raw.startswith("rename to "):
            current.path, current.rename = raw[len("rename to "):], True
        elif raw.startswith("--- "):
            current.old_path = parse_diff_path(raw[4:])
        elif raw.startswith("+++ "):
            current.path = parse_diff_path(raw[4:])
        elif raw.startswith("@@ "):
            hunk = HUNK.match(raw)
            if hunk is None:
                raise ValueError(f"malformed hunk header in {current.path}")
            old, start, new = hunk.groups()
            new_line, old_left, new_left = int(start), int(old or 1), int(new or 1)
        elif raw.startswith(("GIT binary patch", "Binary files ")):
            current.binary = True
    return changes


def parse_commit_map(path: pathlib.Path) -> dict[str, str]:
    if not path.is_file() or path.stat().st_size == 0:
        raise ValueError("intake: mapping: filter-repo emitted no commit-map")
    mapping = {}
    for line in path.read_text().splitlines():
        parts = line.split()
        if len(parts) == 2 and all(re.fullmatch(r"[0-9a-f]{40}", part) for part in parts):
            mapping[parts[0]] = parts[1]
    return mapping


def git(args: list[str], cwd: Optional[pathlib.Path] = None, data: Optional[bytes] = None, check: bool = True) -> subprocess.CompletedProcess:
    return subprocess.run(["git", *args], cwd=cwd, input=data, capture_output=True, check=check)


def git_text(args: list[str], cwd: Optional[pathlib.Path] = None) -> str:
    return git(args, cwd).stdout.decode().strip()


class RealTransport:
    def __init__(self) -> None:
        validate_registry()
        self.temp = tempfile.TemporaryDirectory(prefix="yi-intake-")
        self.work = pathlib.Path(self.temp.name)
        self.source = os.environ.get("MIRROR_SOURCE", f"ssh://git@{forge_pr.HOST}:2222/apex/yi.git")
        self.source_repo = self.work / "source.git"
        self.mirror_repo = self.work / "mirror.git"
        git(["clone", "--quiet", "--bare", "--single-branch", "--branch", "main", "--no-tags", self.source, str(self.source_repo)])
        cloned = subprocess.run(["bash", str(ROOT / "scripts/mirror/rewrite.sh"), self.source, str(self.mirror_repo)], capture_output=True, text=True, check=True).stdout.strip()
        self.current_main = cloned
        git(["-C", str(self.mirror_repo), "fetch", "--quiet", f"https://github.com/{GITHUB_REPO}.git", "main"])
        self.mirror_tip = git_text(["-C", str(self.mirror_repo), "rev-parse", "FETCH_HEAD"])
        self.mirror_to_forge = {mirror: forge for forge, mirror in parse_commit_map(self.mirror_repo / "filter-repo/commit-map").items()}

    def close(self) -> None:
        self.temp.cleanup()

    def github(self, method: str, path: str, payload: Optional[dict[str, Any]] = None) -> Any:
        url = "https://api.github.com/" + path.lstrip("/")
        data = json.dumps(payload).encode() if payload is not None else None
        request = urllib.request.Request(url, data=data, method=method, headers={"Accept": "application/vnd.github+json", "User-Agent": "yi-intake"})
        token = os.environ.get("GITHUB_INTAKE_READ_TOKEN") if method == "GET" else os.environ.get("GITHUB_INTAKE_WRITE_TOKEN")
        if token:
            request.add_header("Authorization", f"Bearer {token}")
        with urllib.request.urlopen(request, timeout=60) as response:
            return json.load(response)

    def forge(self, method: str, path: str, payload: Optional[dict[str, Any]] = None) -> Any:
        return forge_pr.fgj_api(method, path, payload)

    def actor(self) -> str:
        return (forge_pr.fgj_api("GET", "user") or {}).get("login", "")

    def mirror_sha(self, forge_sha: str) -> Optional[str]:
        return {forge: mirror for mirror, forge in self.mirror_to_forge.items()}.get(forge_sha)

    def main_sha(self) -> str:
        return git_text(["-C", str(self.source_repo), "rev-parse", "main"])

    def workflow_equal(self, current: str, imported: str) -> bool:
        return git(["-C", str(self.source_repo), "diff", "--quiet", current, imported, "--", ".forgejo/"], check=False).returncode == 0

    def patch_equal(self, imported: str, head: str) -> bool:
        def branches(sha: str) -> bytes:
            return git(["-C", str(self.source_repo), "diff", "--binary", "--full-index", f"{self.current_main}...{sha}"]).stdout
        return branches(imported) == branches(head)

    def branch_sha(self, branch: str) -> Optional[str]:
        row = self.forge("GET", f"repos/{FORGE_REPO}/branches/{branch}") or {}
        return (row.get("commit") or {}).get("id")

    def push(self, imported: str, branch: str) -> None:
        git(["-C", str(self.source_repo), "push", "--quiet", "origin", f"{imported}:refs/heads/{branch}"])

    def rerun(self, number: int) -> None:
        self.forge("PATCH", f"repos/{FORGE_REPO}/pulls/{number}", {"state": "closed"})
        time.sleep(2)
        self.forge("PATCH", f"repos/{FORGE_REPO}/pulls/{number}", {"state": "open"})

    def apply_and_commit(self, case: Case) -> str:
        work = self.worktree(case.forge_base)
        try:
            applied = git(["-C", str(work), "apply", "--index", "--whitespace=error", "-"], data=case.patch, check=False)
            if applied.returncode:
                raise SystemExit(applied.stderr.decode("utf-8", "replace"))
            email = case.contributor["emails"][0]
            env = dict(os.environ, GIT_AUTHOR_NAME=case.login, GIT_AUTHOR_EMAIL=email, GIT_COMMITTER_NAME="yi-bot", GIT_COMMITTER_EMAIL="yi-bot@localhost")
            message = f"Import GitHub PR #{case.number}\n\nGitHub-Head: {case.head}\nMirror-Base: {case.merge_base}\nForge-Base: {case.forge_base}\n"
            subprocess.run(["git", "-C", str(work), "-c", "core.hooksPath=/dev/null", "commit", "-m", message], env=env, check=True, capture_output=True)
            return git_text(["-C", str(work), "rev-parse", "HEAD"])
        finally:
            git(["-C", str(self.source_repo), "worktree", "remove", "--force", str(work)], check=False)

    def supersede(self, case: Case, number: int) -> None:
        prefix = f"gh/{case.login}/{case.number}-"
        for pull in self.forge("GET", f"repos/{FORGE_REPO}/pulls?state=open&limit=50") or []:
            if ((pull.get("head") or {}).get("ref") or "").startswith(prefix) and pull.get("number") != number:
                self.forge("POST", f"repos/{FORGE_REPO}/issues/{pull['number']}/comments", {"body": f"Superseded by {FORGE_REPO}#{number} for {GITHUB_REPO}#{case.number}."})
                self.forge("PATCH", f"repos/{FORGE_REPO}/pulls/{pull['number']}", {"state": "closed"})

    def signer_file(self, case: Case) -> pathlib.Path:
        path = self.work / f"signers-{case.login}"
        lines = signer_lines(case.contributor, (ROOT / "scripts/intake/allowed_signers").read_text())
        path.write_text("\n".join(lines) + ("\n" if lines else ""))
        return path

    def load_case(self, number: int) -> Case:
        pr = self.github("GET", f"repos/{GITHUB_REPO}/pulls/{number}")
        login = pr.get("user", {}).get("login", "")
        allowed = load_allowed()["contributors"]
        contributor = allowed.get(login, {"emails": [], "caps": {key: 0 for key in CAPS}, "dependencies": False})
        head = pr.get("head", {}).get("sha", "")
        if login not in allowed:
            return Case(number, login, pr.get("state", ""), pr.get("base", {}).get("repo", {}).get("full_name", ""), pr.get("base", {}).get("ref", ""), head, head, head, "", "", self.current_main, [], [], b"", contributor)
        git(["-C", str(self.mirror_repo), "fetch", "--quiet", f"https://github.com/{GITHUB_REPO}.git", f"refs/pull/{number}/head"])
        fetched = git_text(["-C", str(self.mirror_repo), "rev-parse", "FETCH_HEAD"])
        merge_base = git_text(["-C", str(self.mirror_repo), "merge-base", fetched, self.mirror_tip])
        forge_base = self.mirror_to_forge.get(merge_base, merge_base)
        mapped = (forge_base,) if merge_base in self.mirror_to_forge else ()
        ancestor = git(["-C", str(self.mirror_repo), "merge-base", "--is-ancestor", merge_base, self.mirror_tip], check=False).returncode == 0
        commits = []
        rows = git_text(["-C", str(self.mirror_repo), "rev-list", "--reverse", "--parents", f"{merge_base}..{fetched}"]).splitlines()
        signers = self.signer_file(Case(number, login, "open", GITHUB_REPO, "main", head, fetched, head, merge_base, forge_base, self.current_main, [], [], b"", contributor))
        for row in rows:
            parts = row.split()
            sha = parts[0]
            identity = git_text(["-C", str(self.mirror_repo), "show", "-s", "--format=%ae%n%ce", sha]).splitlines()
            commits.append(Commit(sha, identity[0], identity[1], len(parts) - 1, commit_verified(self.mirror_repo, signers, sha)))
        patch = git(["-C", str(self.mirror_repo), "diff", "--no-renames", "--binary", "--full-index", merge_base, fetched]).stdout
        case = Case(number, login, pr.get("state", ""), pr.get("base", {}).get("repo", {}).get("full_name", ""), pr.get("base", {}).get("ref", ""), head, fetched, head, merge_base, forge_base, self.current_main, commits, parse_diff(patch), patch, contributor, mapped, ancestor)
        case.title = pr.get("title") or ""
        case.user_id = pr.get("user", {}).get("id")
        case.body = pr.get("body") or ""
        case.apply_check = self.apply_check
        case.lens = default_lens
        return case

    def worktree(self, base: str) -> pathlib.Path:
        path = pathlib.Path(tempfile.mkdtemp(prefix="apply-", dir=self.work))
        git(["-C", str(self.source_repo), "worktree", "add", "--detach", str(path), base])
        return path

    def apply_check(self, patch: bytes, base: str) -> Optional[str]:
        work = self.worktree(base)
        try:
            out = git(["-C", str(work), "apply", "--check", "--whitespace=error", "-"], data=patch, check=False)
            if out.returncode:
                return (out.stderr or out.stdout).decode("utf-8", "replace")
            return None
        finally:
            git(["-C", str(self.source_repo), "worktree", "remove", "--force", str(work)], check=False)


def default_lens(prompt: str) -> Any:
    return pr_review.ask(prompt, LENS_SCHEMA, ROOT, flags=["--no-tools"])


class DryRunTransport:
    def __init__(self, inner: Any, prohibit_writes: bool = True) -> None:
        self.inner = inner
        self.prohibit_writes = prohibit_writes

    def github(self, method: str, path: str, payload: Optional[dict[str, Any]] = None) -> Any:
        if self.prohibit_writes and method != "GET":
            raise RuntimeError("dry-run write attempted")
        return self.inner.github(method, path, payload)

    def __getattr__(self, name: str) -> Any:
        value = getattr(self.inner, name)
        if self.prohibit_writes and name in ("forge", "apply_and_commit", "push", "rerun", "supersede"):
            raise RuntimeError("dry-run write attempted")
        return value


AUDIT_MARK = "<!-- yi-intake -->"
REBASE = "git fetch https://github.com/jackmaxwil/yi.git main && git rebase FETCH_HEAD"


def owner_guard(transport: Any) -> Optional[Refusal]:
    return None if transport.actor() == "jack" else refusal("owner", "only the owner may admit an external change or land it")


def forge_labels(pull: dict[str, Any]) -> set[str]:
    return {row.get("name") if isinstance(row, dict) else str(row) for row in pull.get("labels") or []}


def audit_comment(transport: Any, number: int) -> Optional[dict[str, Any]]:
    for comment in transport.forge("GET", f"repos/{FORGE_REPO}/issues/{number}/comments") or []:
        body = comment.get("body") or ""
        if body.startswith(AUDIT_MARK):
            return json.loads(body.split("```json", 1)[1].split("```", 1)[0])
    return None


def forge_pull_for(transport: Any, case: Case) -> Optional[dict[str, Any]]:
    for pull in transport.forge("GET", f"repos/{FORGE_REPO}/pulls?state=all&limit=50") or []:
        if (pull.get("head") or {}).get("ref") == branch_name(case):
            return pull
    return None


def admit_run_ci(transport: Any, number: int) -> Any:
    guard = owner_guard(transport)
    if guard:
        return guard
    pull = transport.forge("GET", f"repos/{FORGE_REPO}/pulls/{number}")
    if not pull:
        return refusal("hold", f"{FORGE_REPO}#{number}: no such pull request")
    if "external" not in forge_labels(pull):
        return refusal("hold", f"{FORGE_REPO}#{number} is not external; only an imported change waits for run-ci")
    if pull.get("merged") or pull.get("state") != "open":
        return refusal("hold", f"{FORGE_REPO}#{number} is {pull.get('state')}, not an open pull request")
    head = (pull.get("head") or {}).get("sha") or ""
    try:
        audit = audit_comment(transport, number)
    except (IndexError, ValueError):
        return refusal("hold", f"{FORGE_REPO}#{number} carries an unreadable intake audit comment")
    if not audit or not audit.get("imported"):
        return refusal("hold", f"{FORGE_REPO}#{number} carries no intake audit comment")
    imported = audit["imported"]
    if not transport.patch_equal(imported, head) or not transport.workflow_equal(transport.main_sha(), head):
        return refusal("hold", f"{FORGE_REPO}#{number} head {head} is not audited change {imported} on current main")
    transport.forge("POST", f"repos/{FORGE_REPO}/issues/{number}/labels", {"labels": ["run-ci"]})
    transport.rerun(number)
    return ALLOW


def land_case(transport: Any, case: Case) -> Any:
    pull = forge_pull_for(transport, case)
    if not pull:
        return refusal("land", f"no imported pull request for {branch_name(case)}")
    number = pull.get("number")
    if not pull.get("merged"):
        return refusal("land", f"{FORGE_REPO}#{number} is not merged; GitHub #{case.number} stays open")
    landed = transport.mirror_sha((pull.get("head") or {}).get("sha") or "")
    if not landed:
        return refusal("mapping", f"{FORGE_REPO}#{number} head is not in the mirror commit map")
    body = f"landed as {landed} on Forgejo\n\nThe mirror's main is rewritten, so this pull request is closed rather than marked merged. Rebase onto the new main: `{REBASE}`"
    transport.github("POST", f"repos/{GITHUB_REPO}/issues/{case.number}/comments", {"body": body})
    transport.github("PATCH", f"repos/{GITHUB_REPO}/pulls/{case.number}", {"state": "closed"})
    return ALLOW


def poll(transport: Any) -> int:
    for row in transport.github("GET", f"repos/{GITHUB_REPO}/pulls?state=open&per_page=100"):
        case = transport.load_case(row["number"])
        pull = forge_pull_for(transport, case)
        if pull is None:
            result = import_case(case, transport).decision
        elif pull.get("merged"):
            result = land_case(transport, case)
        else:
            continue
        print(f"PR #{case.number}: {result if isinstance(result, str) else result.text}")
    return 0


def cli(argv: Optional[list[str]] = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--selfcheck", action="store_true")
    parser.add_argument("--dry-run", action="store_true")
    parser.add_argument("--run-ci", type=int)
    parser.add_argument("--land", type=int)
    parser.add_argument("--poll", action="store_true")
    parser.add_argument("number", nargs="?", type=int)
    args = parser.parse_args(argv)
    if args.selfcheck:
        import test_bridge
        return test_bridge.selfcheck()
    transport = RealTransport()
    sealed = DryRunTransport(transport) if args.dry_run or not (args.run_ci or args.land or args.poll or args.number) else transport
    try:
        if args.run_ci:
            decision = admit_run_ci(transport, args.run_ci)
            print(f"{FORGE_REPO}#{args.run_ci}: {decision if isinstance(decision, str) else decision.text}")
            return 0 if decision == ALLOW else 1
        if args.land:
            case = sealed.load_case(args.land)
            decision = land_case(transport, case)
            print(f"PR #{case.number}: {decision if isinstance(decision, str) else decision.text}")
            return 0 if decision == ALLOW else 1
        if args.poll:
            return poll(transport)
        numbers = [args.number] if args.number else [row["number"] for row in transport.github("GET", f"repos/{GITHUB_REPO}/pulls?state=open&per_page=100")]
        for number in numbers:
            case = sealed.load_case(number)
            if sealed is transport:
                result = import_case(case, transport).decision
                print(f"PR #{number}: {result if isinstance(result, str) else result.text}")
            else:
                decision = evaluate(case)
                print(f"PR #{number} {decision if decision == ALLOW else 'REFUSE ' + decision.text}")
        return 0
    finally:
        transport.close()


if __name__ == "__main__":
    raise SystemExit(cli())
