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
    add("hostname-edu", "network-literal", added(base, "example.edu"), "network-literal")
    add("git-url", "network-literal", added(base, "git://host/repo"), "network-literal")
    add("ipv6", "network-literal", added(base, "2001:4860:4860::8888"), "network-literal")
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

def check_rewrite_helper() -> None:
    env = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
    with tempfile.TemporaryDirectory(prefix="intake-rewrite-") as temp:
        source = pathlib.Path(temp) / "source"
        rewritten = pathlib.Path(temp) / "rewritten.git"
        subprocess.run(["git", "init", "-q", "-b", "main", str(source)], check=True)
        (source / "README").write_text("safe fixture\n")
        subprocess.run(["git", "-C", str(source), "add", "README"], check=True)
        subprocess.run(["git", "-C", str(source), "-c", "core.hooksPath=/dev/null", "commit", "-m", "Base"], env=env, check=True, capture_output=True)
        original = subprocess.check_output(["git", "-C", str(source), "rev-parse", "HEAD"], text=True).strip()
        out = subprocess.run(["bash", str(ROOT / "scripts/mirror/rewrite.sh"), str(source), str(rewritten)], capture_output=True, text=True, check=True)
        assert out.stdout.strip() == original, out.stdout
        mirror = subprocess.check_output(["git", "-C", str(rewritten), "rev-parse", "main"], text=True).strip()
        mapping = bridge.parse_commit_map(rewritten / "filter-repo/commit-map")
        assert mapping[original] == mirror
        assert {new: old for old, new in mapping.items()}[mirror] == original


def check_real_patch_edges() -> None:
    env = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
    with tempfile.TemporaryDirectory(prefix="intake-mode-patch-") as temp:
        repo = pathlib.Path(temp)
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / "script.sh").write_text("exit 0\n")
        subprocess.run(["git", "-C", str(repo), "add", "script.sh"], check=True)
        subprocess.run(["git", "-C", str(repo), "-c", "core.hooksPath=/dev/null", "commit", "-m", "Base"], env=env, check=True, capture_output=True)
        (repo / "script.sh").chmod(0o755)
        patch = subprocess.check_output(["git", "-C", str(repo), "diff", "--no-renames", "--binary", "--full-index"])
        case = dataclasses.replace(valid_case(), changes=bridge.parse_diff(patch), patch=patch)
        assert bridge.check_13_executables(case).text == "intake: mode: 'script.sh' changes the executable bit"
    with tempfile.TemporaryDirectory(prefix="intake-binary-patch-") as temp:
        repo = pathlib.Path(temp)
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        (repo / "blob.dat").write_bytes(b"\x00\x01\x02")
        subprocess.run(["git", "-C", str(repo), "add", "blob.dat"], check=True)
        patch = subprocess.check_output(["git", "-C", str(repo), "diff", "--cached", "--no-renames", "--binary", "--full-index"])
        case = dataclasses.replace(valid_case(), changes=bridge.parse_diff(patch), patch=patch)
        assert bridge.check_14_binary(case).text == "intake: binary: 'blob.dat' is binary"



def real_diff(before: dict[str, str], after: dict[str, str]) -> bytes:
    env = dict(os.environ, GIT_AUTHOR_NAME="Fixture", GIT_AUTHOR_EMAIL="fixture@example.invalid", GIT_COMMITTER_NAME="Fixture", GIT_COMMITTER_EMAIL="fixture@example.invalid")
    with tempfile.TemporaryDirectory(prefix="intake-forged-") as temp:
        def git(*args: str) -> bytes:
            return subprocess.run(["git", "-C", temp, *args], env=env, check=True, capture_output=True).stdout

        def write(files: dict[str, str]) -> None:
            for name, text in files.items():
                (pathlib.Path(temp) / name).parent.mkdir(parents=True, exist_ok=True)
                (pathlib.Path(temp) / name).write_bytes(text.encode())
            git("add", "-A")

        git("init", "-q")
        write(before)
        git("-c", "core.hooksPath=/dev/null", "commit", "--allow-empty", "-qm", "Base")
        write(after)
        return git("diff", "--cached", "--no-renames", "--binary", "--full-index")


def check_forged_headers() -> None:
    workflow = ".forgejo/workflows/x.yml"
    spaced = ".forgejo/workflows/ b/ok.yml"
    lines = "".join(f"{number}\n" for number in range(30))
    forged = {
        "deleted line reads as a --- header": ({workflow: "a\n-- docs/x\nb\n"}, {workflow: "a\nb\n"}, [(workflow, [], 1)]),
        "added line reads as a +++ header": ({workflow: "a\nb\n"}, {workflow: "a\n++ docs/ok.md\nb\n"}, [(workflow, ["++ docs/ok.md"], 0)]),
        "both header lines forged, the bypass": ({workflow: "a\n-- docs/x\nb\n"}, {workflow: "a\nb\n++ docs/ok.md\n"}, [(workflow, ["++ docs/ok.md"], 1)]),
        "form feed splits an added line": ({workflow: "a\n"}, {workflow: "a\x0c+++ b/docs/ok.md\n"}, [(workflow, ["a\x0c+++ b/docs/ok.md"], 1)]),
        "two hunks, the second ending without a newline": ({workflow: lines}, {workflow: lines.replace("\n1\n", "\nx\n").replace("\n28\n", "\n-- y\n").rstrip("\n")}, [(workflow, ["x", "-- y", "29"], 3)]),
        "empty file under a ' b' directory": ({}, {spaced: ""}, [(spaced, [], 0)]),
    }
    for name, (before, after, expected) in forged.items():
        patch = real_diff(before, after)
        changes = bridge.parse_diff(patch)
        assert [(c.path, [line.text for line in c.added], c.deleted) for c in changes] == expected, f"{name}: {changes}"
        refused = bridge.check_10_deny_paths(dataclasses.replace(valid_case(), changes=changes, patch=patch))
        assert isinstance(refused, bridge.Refusal) and refused.text.endswith("is denied by .forgejo/"), f"{name}: {refused}"


def check_real_fixture() -> None:
    pr = json.loads((FIXTURES / "pull-request.json").read_text())
    commits = json.loads((FIXTURES / "commits.json").read_text())
    assert pr["head"]["sha"] == H and pr["base"]["sha"] == M
    assert commits and all("verification" in row["commit"] for row in commits)
    history = json.loads((FIXTURES / "mirror-history.json").read_text())
    mapping = bridge.parse_commit_map(FIXTURES / "commit-map.txt")
    for row in history["commits"]:
        assert mapping[row["forge"]] == row["mirror"]
    comparisons = json.loads((FIXTURES / "mirror-files.json").read_text())["comparisons"]
    assert comparisons
    for row in comparisons:
        assert row["mode_equal"], row["path"]
        assert row["expected_sha256"] == row["mirror_sha256"], row["path"]
        assert row["bytes_equal"] or row["replacement_applied"], row["path"]
    assert any(row["bytes_equal"] for row in comparisons)
    assert any(row["replacement_applied"] for row in comparisons)


def check_signed_object() -> None:
    raw = (FIXTURES / "signed-commit-object.txt").read_bytes()
    public = (FIXTURES / "signed-commit-key.pub").read_text().strip()
    with tempfile.TemporaryDirectory(prefix="intake-signed-object-") as temp:
        repo = pathlib.Path(temp)
        subprocess.run(["git", "init", "-q", str(repo)], check=True)
        empty = subprocess.run(["git", "-C", str(repo), "mktree"], input=b"", capture_output=True, check=True).stdout
        sha = subprocess.run(["git", "-C", str(repo), "hash-object", "-t", "commit", "-w", "--stdin"], input=raw, capture_output=True, check=True).stdout.decode().strip()
        assert empty.decode().strip() == raw.split(b"\n", 1)[0].split()[1].decode()
        contributor = {"emails": ["intake-fixture@example.invalid"], "fingerprints": [], "caps": {"commits": 20, "files": 40, "lines": 4000, "bytes": 150000}, "dependencies": False}
        registry = "intake-fixture@example.invalid " + public + "\n"
        signers = repo / "allowed_signers"
        signers.write_text("\n".join(bridge.signer_lines(contributor, registry)) + "\n")
        fingerprint = subprocess.check_output(["ssh-keygen", "-lf", FIXTURES / "signed-commit-key.pub"], text=True).split()[1]
        contributor["fingerprints"] = [fingerprint]
        allowed = repo / "allowed.json"
        allowed.write_text(json.dumps({"contributors": {"fixture": contributor}}))
        bridge.validate_registry(allowed, signers)
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
    for call in (lambda: dry.github("POST", "repos/x/issues/1/comments", {}), lambda: dry.push("abc", "gh/jackmaxwil/1-abc")):
        try:
            call()
        except RuntimeError as error:
            assert str(error) == "dry-run write attempted"
        else:
            raise AssertionError("dry-run write prohibition did not fire")
    unguarded = bridge.DryRunTransport(fake, prohibit_writes=False)
    unguarded.push("abc", "gh/jackmaxwil/1-abc")
    assert fake.pushes == [("abc", "gh/jackmaxwil/1-abc")]


def check_overrides() -> None:
    base = valid_case()
    cap = base.contributor["caps"]["commits"]
    case = dataclasses.replace(base, commits=many_commits(base, cap + 1))
    assert bridge.evaluate(case, override="commits") == bridge.ALLOW
    denied = with_change(base, path="scripts/intake/allowed.json", old_path="scripts/intake/allowed.json")
    assert bridge.evaluate(denied, override="path").code == "path"
    assert bridge.evaluate(case, override="signature").code == "commits"


EXPECTED = {
    "stranger": "intake: allowlist: GitHub login 'stranger' is not allowed",
    "base-shape": "intake: base: PR #11636 targets octocat/Hello-World:master, expected jackmaxwil/yi:main",
    "unsigned": f"intake: signature: commit {H} has no signature verifiable by jackmaxwil's pinned keys",
    "wrong-author": f"intake: identity: commit {H} author 'other@example.invalid' or committer 'mazac@chapman.edu' is not pinned to jackmaxwil",
    "fetch-pin": f"intake: head: fetched {'0' * 40}, expected pinned {H}",
    "moved-head": f"intake: head: PR #11636 moved from {H} to {'0' * 40}; no branch pushed",
    "commits-cap-plus-one": "intake: commits: 21 exceeds per-login cap 20",
    "files-cap-plus-one": "intake: files: 41 exceeds per-login cap 40",
    "lines-cap-plus-one": "intake: lines: 4001 exceeds per-login cap 4000",
    "bytes-cap-plus-one": "intake: bytes: 150001 exceeds per-login cap 150000",
    "unmappable-base": f"intake: base: mirror merge-base {M} has no unique Forgejo mapping",
    "merge-commit": f"intake: history: merge commit {H} is not importable; rebase into linear signed commits",
    "deny-path": "intake: path: 'scripts/intake/allowed.json' is denied by scripts/intake/",
    "symlink": "intake: mode: 'README' is a symlink",
    "submodule": "intake: mode: 'README' is a submodule",
    "executable": "intake: mode: 'README' changes the executable bit",
    "binary": "intake: binary: 'README' is binary",
    "lfs": "intake: lfs: 'README' is an LFS pointer",
    "rename-side": "intake: path: rename side 'scripts/intake/allowed.json' is denied by scripts/intake/",
    "dot-dot": "intake: filename: 'docs/../escape' fails empty, '.' or '..' component",
    "case-fold": "intake: filename: 'Readme' fails case-fold collision with 'README'",
    "secret": "intake: content: 'README:2' matches secret/private pattern deny.txt:15",
    "placeholder": "intake: content: 'README:2' contains mirror replacement marker placeholder",
    "replacement": "intake: content: 'README:2' contains mirror replacement marker replace.txt:4",
    "url": "intake: network-literal: 'README:2' contains URL; owner review is required",
    "ip": "intake: network-literal: 'README:2' contains IP literal; owner review is required",
    "unsafe": "intake: unsafe: 'README:2' adds unsafe",
    "proc-macro": "intake: build: 'crates/probe/Cargo.toml:2' adds build-time execution proc-macro",
    "dependency-section": "intake: build: 'Cargo.toml:2' adds build-time execution dependency section",
    "dependency": "intake: dependencies: 'Cargo.toml' changes dependencies without per-login permission",
    "replaced-context": f"intake: apply: patch does not apply strictly to Forgejo base {F}: error: patch failed: README:1",
    "invalid-lens": "intake: lens unavailable: invalid lens output",
    "lens-refusal": "intake: lens: model-added refusal",
}


def selfcheck() -> int:
    assert len(bridge.CHECKS) == 25
    check_real_fixture()
    check_rewrite_helper()
    check_real_patch_edges()
    check_forged_headers()
    check_signed_object()
    valid = valid_case()
    assert bridge.evaluate(valid) == bridge.ALLOW
    check_caps()
    red = []
    for name, check, case, code in hostile_cases():
        decision = bridge.evaluate(case)
        assert isinstance(decision, bridge.Refusal), f"{name}: expected refusal, got {decision}"
        assert decision.text.startswith(f"intake: {code}: "), decision.text
        if name in EXPECTED:
            assert decision.text == EXPECTED[name], f"{name}: {decision.text}"
        without = bridge.evaluate(case, disabled=frozenset({check}))
        assert without != decision, f"{name}: disabling {check} kept {decision.text}"
        shown = without if isinstance(without, str) else without.text
        red.append(f"{name} · disable {check}: expected {decision.text}; got {shown}")
    check_dry_run_write_detection()
    check_overrides()
    assert bridge.server_import_refusal().text == "intake: import: server writes stay disabled until stage 3 installs the run-ci hold"
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
