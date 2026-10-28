#!/usr/bin/env python3
import dataclasses
import json
import os
import pathlib
import subprocess
import sys
import tempfile

import bridge
import test_bridge


class FakeTransport:
    def __init__(self, case: bridge.Case, repo: pathlib.Path) -> None:
        self.case = case
        self.repo = repo
        self.github_writes = []
        self.forge_calls = []
        self.pushes = []
        self.imported = None

    def github(self, method: str, path: str, payload=None):
        if method == "GET":
            return {"head": {"sha": self.case.head}}
        self.github_writes.append((method, path, payload))
        return {}

    def forge(self, method: str, path: str, payload=None):
        self.forge_calls.append((method, path, payload))
        if path.endswith("/pulls"):
            return {"number": 42}
        return {}

    def actor(self) -> str:
        return "jack"

    def apply_and_commit(self, case: bridge.Case) -> str:
        checked = subprocess.run(["git", "-C", str(self.repo), "apply", "--check", "--whitespace=error", "-"], input=case.patch, capture_output=True, check=False)
        assert checked.returncode == 0, checked.stderr.decode()
        applied = subprocess.run(["git", "-C", str(self.repo), "apply", "--index", "--whitespace=error", "-"], input=case.patch, capture_output=True, check=False)
        assert applied.returncode == 0, applied.stderr.decode()
        staged = subprocess.check_output(["git", "-C", str(self.repo), "diff", "--cached", "--no-renames", "--binary", "--full-index"])
        assert bridge.change_signature(bridge.parse_diff(staged)) == bridge.change_signature(case.changes)
        env = dict(os.environ, GIT_AUTHOR_NAME="Recorded Author", GIT_AUTHOR_EMAIL=case.contributor["emails"][0], GIT_COMMITTER_NAME="yi-bot", GIT_COMMITTER_EMAIL="yi-bot@localhost", GIT_AUTHOR_DATE="2026-10-28T00:00:00Z", GIT_COMMITTER_DATE="2026-10-28T00:00:00Z")
        message = f"Import GitHub PR #{case.number}\n\nGitHub-Head: {case.head}\nMirror-Base: {case.merge_base}\nForge-Base: {case.forge_base}\n"
        subprocess.run(["git", "-C", str(self.repo), "-c", "core.hooksPath=/dev/null", "commit", "-m", message], env=env, check=True, capture_output=True)
        self.imported = subprocess.check_output(["git", "-C", str(self.repo), "rev-parse", "HEAD"], text=True).strip()
        return self.imported

    def main_sha(self) -> str:
        return self.case.current_main

    def workflow_equal(self, current: str, imported: str) -> bool:
        return subprocess.run(["git", "-C", str(self.repo), "diff", "--quiet", current, imported, "--", ".forgejo/"], check=False).returncode == 0

    def branch_sha(self, branch: str):
        out = subprocess.run(["git", "-C", str(self.repo), "rev-parse", "--verify", f"refs/heads/{branch}"], capture_output=True, text=True, check=False)
        return out.stdout.strip() if out.returncode == 0 else None

    def push(self, imported: str, branch: str) -> None:
        subprocess.run(["git", "-C", str(self.repo), "update-ref", f"refs/heads/{branch}", imported], check=True)
        self.pushes.append((imported, branch))

    def supersede(self, case: bridge.Case, new_number: int) -> None:
        self.forge_calls.append(("SUPERSEDE", str(case.number), new_number))

    def run_ci(self) -> None:
        raise AssertionError("external import started CI without an owner action")


def repository(case: bridge.Case) -> tuple[pathlib.Path, str]:
    repo = pathlib.Path(tempfile.mkdtemp(prefix="intake-journey-git-"))
    subprocess.run(["git", "init", "-q", str(repo)], check=True)
    (repo / "README").write_text("Hello World!\n")
    subprocess.run(["git", "-C", str(repo), "add", "README"], check=True)
    env = dict(os.environ, GIT_AUTHOR_NAME="Base", GIT_AUTHOR_EMAIL="base@example.invalid", GIT_COMMITTER_NAME="Base", GIT_COMMITTER_EMAIL="base@example.invalid")
    subprocess.run(["git", "-C", str(repo), "-c", "core.hooksPath=/dev/null", "commit", "-m", "Base"], env=env, check=True, capture_output=True)
    base = subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()
    case.forge_base = base
    case.current_main = base
    case.apply_check = lambda patch, _base: subprocess.run(["git", "-C", str(repo), "apply", "--check", "--whitespace=error", "-"], input=patch, capture_output=True, check=False).stderr.decode() or None
    return repo, base


def main() -> int:
    refused = dataclasses.replace(test_bridge.valid_case(), login="stranger")
    decision = bridge.evaluate(refused)
    assert isinstance(decision, bridge.Refusal) and decision.code == "allowlist"
    line = f"PR #{refused.number} REFUSE {decision.text}"
    assert line == f"PR #{refused.number} REFUSE intake: allowlist: GitHub login 'stranger' is not allowed"

    case = test_bridge.valid_case()
    repo, base = repository(case)
    transport = FakeTransport(case, repo)
    dry = bridge.DryRunTransport(transport)
    try:
        dry.github("POST", "repos/jackmaxwil/yi/issues/1/comments", {})
    except RuntimeError as error:
        assert str(error) == "dry-run write attempted"
    else:
        raise AssertionError("dry-run accepted a write")
    assert not transport.github_writes and not transport.forge_calls and not transport.pushes

    result = bridge.import_case(case, transport)
    assert result.decision == bridge.ALLOW
    expected_branch = f"gh/jackmaxwil/{case.number}-{case.head[:8]}"
    assert result.branch == expected_branch and transport.pushes == [(transport.imported, expected_branch)]
    assert subprocess.check_output(["git", "-C", str(repo), "rev-parse", expected_branch], text=True).strip() == transport.imported
    assert "## About This Project" in (repo / "README").read_text()
    assert result.draft == bridge.draft_payload(case)
    assert result.draft["title"] == f"WIP: External GitHub PR #{case.number} {case.head[:8]}"
    assert result.draft["draft"] is True and 'trust="untrusted"' in result.draft["body"]
    labels = next(call[2]["labels"] for call in transport.forge_calls if call[1].endswith("/labels"))
    assert labels == ["external", "autofix:hold"]
    assert not {"run-ci", "live", "autofix"} & set(labels)
    assert result.audit["head"] == case.head and result.audit["forge_base"] == base
    assert result.audit["imported"] == transport.imported and result.audit["actor"] == "jack"
    assert result.audit["github_pr"] == case.number and result.audit["mirror_merge_base"] == case.merge_base
    assert len(result.audit["checks"]) == 25 and all(row["decision"] == "ALLOW" for row in result.audit["checks"])
    assert any(call[1].endswith("/issues/42/comments") for call in transport.forge_calls)
    assert transport.github_writes[-1][2]["body"].endswith(json.dumps(result.audit, indent=2, sort_keys=True) + "\n```")

    overridden = dataclasses.replace(case, commits=test_bridge.many_commits(case, case.contributor["caps"]["commits"] + 1))
    second_repo, _ = repository(overridden)
    second = FakeTransport(overridden, second_repo)
    override = {"check": "commits", "sha": overridden.head, "reason": "owner reviewed the extra commit"}
    accepted = bridge.import_case(overridden, second, override)
    assert accepted.decision == bridge.ALLOW
    assert accepted.audit["override"] == override
    assert accepted.audit["original_refusal"] == "intake: commits: 21 exceeds per-login cap 20"
    print(line)
    print(f"ALLOW {expected_branch} draft=42 labels={','.join(labels)} run-ci=held strict-base={base}")
    print("ok   intake journey (real git, fake HTTP, zero network)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
