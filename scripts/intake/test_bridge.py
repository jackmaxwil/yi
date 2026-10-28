#!/usr/bin/env python3
import copy
import dataclasses
import json
import os
import pathlib
import subprocess
import sys
import tempfile

import bridge

ROOT = bridge.ROOT
FIXTURES = ROOT / "scripts/intake/fixtures"
H = "16991ef417f2f1f5ab7ae4029ab3a4c373360907"
M = "7fd1a60b01f91b314f59955a4e4d4e80d8edf11d"
F = "53c7af144af22d8460e45f96d6337ecba4470d8f"
TEMPS = []


def valid_case() -> bridge.Case:
    recorded = json.loads((FIXTURES / "pull-request.json").read_text())
    raw = (FIXTURES / "pull-request.patch").read_bytes()
    patch = b"diff --git " + raw.split(b"diff --git ", 1)[1]
    contributor = copy.deepcopy(bridge.load_allowed()["contributors"]["jackmaxwil"])
    base = tempfile.TemporaryDirectory(prefix="intake-valid-base-")
    path = pathlib.Path(base.name)
    context = next(line[1:] for line in patch.decode().splitlines() if line.startswith(" Hello World!"))
    (path / "README").write_text(context + "\n")
    TEMPS.append(base)
    def apply_check(data: bytes, _base: str):
        out = subprocess.run(["git", "apply", "--check", "--whitespace=error", "-"], cwd=path, input=data, capture_output=True, check=False)
        if out.returncode:
            return out.stderr.decode("utf-8", "replace")
        return None

    case = bridge.Case(
        recorded["number"], "jackmaxwil", "open", bridge.GITHUB_REPO, "main", H, H, H, M, F, F,
        [bridge.Commit(H, "mazac@chapman.edu", "mazac@chapman.edu")], bridge.parse_diff(patch), patch, contributor,
    )
    case.title = recorded["title"]
    case.body = recorded["body"]
    case.apply_check = apply_check
    case._fixture_temp = base
    return case


def with_change(case: bridge.Case, **changes) -> bridge.Case:
    cloned = copy.deepcopy(case)
    for key, value in changes.items():
        setattr(cloned.changes[0], key, value)
    return cloned


def added(case: bridge.Case, text: str, path: str = "README") -> bridge.Case:
    cloned = copy.deepcopy(case)
    cloned.changes = [bridge.Change(path, added=[bridge.AddedLine(path, 2, text)])]
    return cloned


def many_commits(case: bridge.Case, count: int) -> list[bridge.Commit]:
    return [bridge.Commit(f"{index:040x}", "mazac@chapman.edu", "mazac@chapman.edu") for index in range(1, count + 1)]


def hostile_cases() -> list[tuple[str, str, bridge.Case, str]]:
    base = valid_case()
    cap = base.contributor["caps"]
    cases = []

    def add(name: str, check: str, case: bridge.Case, code: str) -> None:
        cases.append((name, check, case, code))

    add("stranger", "allowlist", dataclasses.replace(base, login="stranger"), "allowlist")
    add("base-shape", "allowlist", dataclasses.replace(base, base_repo="octocat/Hello-World", base_ref="master"), "base")
    add("unsigned", "signature", dataclasses.replace(base, commits=[dataclasses.replace(base.commits[0], signature=False)]), "signature")
    add("wrong-signer", "signature", dataclasses.replace(base, commits=[dataclasses.replace(base.commits[0], signature=False)]), "signature")
    add("wrong-author", "identity", dataclasses.replace(base, commits=[dataclasses.replace(base.commits[0], author="other@example.invalid")]), "identity")
    add("wrong-committer", "identity", dataclasses.replace(base, commits=[dataclasses.replace(base.commits[0], committer="other@example.invalid")]), "identity")
    add("fetch-pin", "head", dataclasses.replace(base, fetched_head="0" * 40), "head")
    add("moved-head", "head", dataclasses.replace(base, current_head="0" * 40), "head")
    add("commits-cap-plus-one", "commits", dataclasses.replace(base, commits=many_commits(base, cap["commits"] + 1)), "commits")
    changes = [bridge.Change(f"docs/file-{index}.md") for index in range(cap["files"] + 1)]
    add("files-cap-plus-one", "files", dataclasses.replace(base, changes=changes), "files")
    long_lines = [bridge.AddedLine("README", index + 2, "safe") for index in range(cap["lines"] + 1)]
    add("lines-cap-plus-one", "lines", dataclasses.replace(base, changes=[bridge.Change("README", added=long_lines)]), "lines")
    add("bytes-cap-plus-one", "bytes", dataclasses.replace(base, patch=b"x" * (cap["bytes"] + 1)), "bytes")
    add("unmappable-base", "base-history", dataclasses.replace(base, mapped_bases=()), "base")
    add("merge-commit", "base-history", dataclasses.replace(base, commits=[dataclasses.replace(base.commits[0], parents=2)]), "history")
    add("deny-path", "path", with_change(base, path="scripts/intake/allowed.json", old_path="scripts/intake/allowed.json"), "path")
    add("build-rs-path", "path", with_change(base, path="crates/probe/build.rs", old_path="/dev/null", old_mode=None), "path")
    add("symlink", "symlink", with_change(base, new_mode="120000"), "mode")
    add("submodule", "submodule", with_change(base, new_mode="160000"), "mode")
    add("executable", "executable", with_change(base, new_mode="100755"), "mode")
    add("binary", "binary", with_change(base, binary=True), "binary")
    add("lfs", "lfs", added(base, "version https://git-lfs.github.com/spec/v1"), "lfs")
    add("rename-side", "rename", with_change(base, rename=True, old_path="README", path="scripts/intake/allowed.json"), "path")
    add("dot-dot", "filename", with_change(base, path="docs/../escape", old_path="docs/../escape"), "filename")
    add("case-fold", "filename", dataclasses.replace(base, changes=base.changes + [bridge.Change("Readme")]), "filename")
    add("non-ascii", "filename", with_change(base, path="docs/ångström.md", old_path="docs/ångström.md"), "filename")
    add("secret", "secret", added(base, "ghp_" + "A" * 30), "content")
    add("placeholder", "marker", added(base, "***REMOVED***"), "content")
    add("replacement", "marker", added(base, "the infra repository"), "content")
    add("url", "network-literal", added(base, "https://example.com/path"), "network-literal")
    add("ip", "network-literal", added(base, "8.8.8.8"), "network-literal")
    add("unsafe", "unsafe", added(base, "unsafe { call(); }"), "unsafe")
    add("proc-macro", "build", added(base, "proc-macro = true", "crates/probe/Cargo.toml"), "build")
    add("dependency-section", "build", added(base, "[dependencies]", "Cargo.toml"), "build")
    add("dependency", "dependencies", added(base, "serde = \"1\"", "Cargo.toml"), "dependencies")

    def failed_apply(_patch: bytes, _base: str):
        return "error: patch failed: README:1"

    add("replaced-context", "apply", dataclasses.replace(base, apply_check=failed_apply), "apply")
    add("invalid-lens", "lens", dataclasses.replace(base, lens=lambda _prompt: {"refusals": "wrong"}), "lens unavailable")
    refusal = {"refusals": [{"path": "README", "line": 2, "reason": "model-added refusal"}]}
    add("lens-refusal", "lens", dataclasses.replace(base, lens=lambda _prompt: refusal), "lens")
    return cases


def check_caps() -> None:
    base = valid_case()
    cap = base.contributor["caps"]
    at_cap = [
        dataclasses.replace(base, commits=many_commits(base, cap["commits"])),
        dataclasses.replace(base, changes=[bridge.Change(f"docs/file-{index}.md") for index in range(cap["files"])]),
        dataclasses.replace(base, changes=[bridge.Change("README", added=[bridge.AddedLine("README", index + 2, "safe") for index in range(cap["lines"])])]),
        dataclasses.replace(base, patch=b"x" * cap["bytes"], apply_check=lambda _patch, _base: None),
    ]
    for case in at_cap:
        assert bridge.evaluate(case) == bridge.ALLOW


def check_real_fixture() -> None:
    pr = json.loads((FIXTURES / "pull-request.json").read_text())
    commits = json.loads((FIXTURES / "commits.json").read_text())
    assert pr["head"]["sha"] == H and pr["base"]["sha"] == M
    assert commits and all("verification" in row["commit"] for row in commits)
    history = json.loads((FIXTURES / "mirror-history.json").read_text())
    mapping = bridge.parse_commit_map(FIXTURES / "commit-map.txt")
    for row in history["commits"]:
        assert mapping[row["forge"]] == row["mirror"]
        assert len(row["forge_tree"]) == len(row["mirror_tree"]) == 40


def check_signed_object() -> None:
    raw = (FIXTURES / "signed-commit-object.txt").read_bytes()
    public = (FIXTURES / "signed-commit-key.pub").read_text().strip()
    with tempfile.TemporaryDirectory(prefix="intake-signed-object-") as temp:
        repo = pathlib.Path(temp)
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        empty = subprocess.run(["git", "-C", str(repo), "mktree"], input=b"", capture_output=True, check=True).stdout
        sha = subprocess.run(["git", "-C", str(repo), "hash-object", "-t", "commit", "-w", "--stdin"], input=raw, capture_output=True, check=True).stdout.decode().strip()
        assert empty.decode().strip() == raw.split(b"\n", 1)[0].split()[1].decode()
        signers = repo / "allowed_signers"
        signers.write_text("intake-fixture@example.invalid " + public + "\n")
        verified = subprocess.run(["git", "-C", str(repo), "-c", "gpg.format=ssh", "-c", f"gpg.ssh.allowedSignersFile={signers}", "verify-commit", sha], capture_output=True, check=False)
        assert verified.returncode == 0, verified.stderr.decode()
        generated = repo / "throwaway"
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(generated)], check=True)
        wrong = repo / "wrong_signers"
        wrong.write_text("intake-fixture@example.invalid " + (generated.with_suffix(".pub")).read_text())
        refused = subprocess.run(["git", "-C", str(repo), "-c", "gpg.format=ssh", "-c", f"gpg.ssh.allowedSignersFile={wrong}", "verify-commit", sha], capture_output=True, check=False)
        assert refused.returncode != 0


def check_dry_run_write_detection() -> None:
    class Fake:
        def __init__(self):
            self.pushes = []

        def github(self, method, path, payload=None):
            return {"method": method, "path": path, "payload": payload}

        def push(self, imported, branch):
            self.pushes.append((imported, branch))

    fake = Fake()
    dry = bridge.DryRunTransport(fake)
    try:
        dry.github("POST", "repos/x/issues/1/comments", {})
    except RuntimeError as error:
        assert str(error) == "dry-run write attempted"
    else:
        raise AssertionError("dry-run write prohibition did not fire")
    fake.push("abc", "gh/jackmaxwil/1-abc")
    assert fake.pushes == [("abc", "gh/jackmaxwil/1-abc")]


def check_overrides() -> None:
    base = valid_case()
    cap = base.contributor["caps"]["commits"]
    case = dataclasses.replace(base, commits=many_commits(base, cap + 1))
    assert bridge.evaluate(case, override="commits") == bridge.ALLOW
    denied = with_change(base, path="scripts/intake/allowed.json", old_path="scripts/intake/allowed.json")
    assert bridge.evaluate(denied, override="path").code == "path"
    assert bridge.evaluate(case, override="signature").code == "commits"


def selfcheck() -> int:
    assert len(bridge.CHECKS) == 25
    check_real_fixture()
    check_signed_object()
    valid = valid_case()
    assert bridge.evaluate(valid) == bridge.ALLOW
    check_caps()
    red = []
    for name, check, case, code in hostile_cases():
        decision = bridge.evaluate(case)
        assert isinstance(decision, bridge.Refusal), f"{name}: expected refusal, got {decision}"
        assert decision.code == code, f"{name}: expected {code}, got {decision.text}"
        assert decision.text.startswith(f"intake: {code}: "), decision.text
        without = bridge.evaluate(case, disabled=frozenset({check}))
        assert without != decision, f"{name}: disabling {check} kept {decision.text}"
        shown = without if isinstance(without, str) else without.text
        red.append(f"{name} · disable {check}: expected {decision.text}; got {shown}")
    check_dry_run_write_detection()
    check_overrides()
    payload = bridge.draft_payload(valid)
    labels = {"external", "autofix:hold"}
    assert labels == {"external", "autofix:hold"}
    assert not labels & {"run-ci", "live", "autofix"}
    assert bridge.branch_name(valid) == f"gh/jackmaxwil/{valid.number}-{H[:8]}"
    assert payload["draft"] is True and payload["head"] == bridge.branch_name(valid)
    assert 'trust="untrusted"' in payload["body"]
    print("\n".join("seen-red " + line for line in red))
    print(f"ok   intake selfcheck ({len(red)} hostile cases, 25 ordered checks, real fixtures)")
    return 0


if __name__ == "__main__":
    raise SystemExit(selfcheck())
