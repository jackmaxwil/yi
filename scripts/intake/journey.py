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


def change_signature(changes: list[bridge.Change]) -> list:
    return [(c.old_path, c.path, c.old_mode, c.new_mode, c.binary, c.rename, len(c.added), c.deleted) for c in changes]


class FakeTransport:
    def __init__(self, case: bridge.Case, repo: pathlib.Path) -> None:
        self.case = case
        self.repo = repo
        self.main = case.current_main
        self.github_writes = []
        self.forge_calls = []
        self.pushes = []
        self.imported = None
        self.login = "jack"
        self.pulls = {}
        self.comments = {}
        self.reruns = []
        self.mirror = {}
        self.workflow_same = True

    def github(self, method: str, path: str, payload=None):
        if method == "GET":
            return {"head": {"sha": self.case.head}}
        self.github_writes.append((method, path, payload))
        return {}

    def forge(self, method: str, path: str, payload=None):
        self.forge_calls.append((method, path, payload))
        base = path.split("?", 1)[0]
        if base.endswith("/pulls") and method == "GET":
            return list(self.pulls.values())
        if base.endswith("/pulls") and method == "POST":
            number = 42
            self.pulls[number] = {"number": number, "head": {"ref": payload["head"], "sha": self.imported}, "labels": [], "state": "open", "merged": False}
            return {"number": number}
        if "/issues/" in base and base.endswith("/labels") and method == "POST":
            self.pulls[int(base.split("/issues/")[1].split("/")[0])]["labels"].extend({"name": name} for name in payload["labels"])
            return {}
        if "/issues/" in base and base.endswith("/comments"):
            number = int(base.split("/issues/")[1].split("/")[0])
            if method == "GET":
                return self.comments.get(number, [])
            self.comments.setdefault(number, []).append({"body": payload["body"]})
            return {}
        if "/pulls/" in base:
            number = int(base.rsplit("/", 1)[1])
            pull = self.pulls.get(number)
            if method == "GET":
                return pull
            if method == "PATCH" and pull is not None:
                pull["state"] = (payload or {}).get("state", pull["state"])
                return pull
        return {}

    def labels(self, number: int) -> list:
        return [row["name"] for row in self.pulls[number]["labels"]]

    def actor(self) -> str:
        return self.login

    def apply_and_commit(self, case: bridge.Case) -> str:
        checked = subprocess.run(["git", "-C", str(self.repo), "apply", "--check", "--whitespace=error", "-"], input=case.patch, capture_output=True, check=False)
        assert checked.returncode == 0, checked.stderr.decode()
        applied = subprocess.run(["git", "-C", str(self.repo), "apply", "--index", "--whitespace=error", "-"], input=case.patch, capture_output=True, check=False)
        assert applied.returncode == 0, applied.stderr.decode()
        staged = subprocess.check_output(["git", "-C", str(self.repo), "diff", "--cached", "--no-renames", "--binary", "--full-index"])
        assert change_signature(bridge.parse_diff(staged)) == change_signature(case.changes)
        identity = bridge.import_identity(case)
        env = dict(os.environ, GIT_AUTHOR_NAME=identity["name"], GIT_AUTHOR_EMAIL=identity["email"], GIT_COMMITTER_NAME="yi-bot", GIT_COMMITTER_EMAIL="yi-bot@localhost", GIT_AUTHOR_DATE="2026-10-28T00:00:00Z", GIT_COMMITTER_DATE="2026-10-28T00:00:00Z")
        subprocess.run(["git", "-C", str(self.repo), "-c", "core.hooksPath=/dev/null", "commit", "-m", bridge.imported_message(case)], env=env, check=True, capture_output=True)
        self.imported = subprocess.check_output(["git", "-C", str(self.repo), "rev-parse", "HEAD"], text=True).strip()
        return self.imported

    def main_sha(self) -> str:
        return self.case.current_main

    def workflow_equal(self, current: str, imported: str) -> bool:
        if not self.workflow_same:
            return False
        return subprocess.run(["git", "-C", str(self.repo), "diff", "--quiet", current, imported, "--", ".forgejo/"], check=False).returncode == 0

    def patch_equal(self, imported: str, head: str) -> bool:
        def branches(sha: str) -> bytes:
            return subprocess.run(["git", "-C", str(self.repo), "diff", "--binary", f"{self.main}...{sha}"], capture_output=True, check=True).stdout
        return branches(imported) == branches(head)

    def branch_sha(self, branch: str):
        out = subprocess.run(["git", "-C", str(self.repo), "rev-parse", "--verify", f"refs/heads/{branch}"], capture_output=True, text=True, check=False)
        return out.stdout.strip() if out.returncode == 0 else None

    def push(self, imported: str, branch: str) -> None:
        subprocess.run(["git", "-C", str(self.repo), "update-ref", f"refs/heads/{branch}", imported], check=True)
        self.pushes.append((imported, branch))

    def supersede(self, case: bridge.Case, new_number: int) -> None:
        self.forge_calls.append(("SUPERSEDE", str(case.number), new_number))

    def mirror_sha(self, forge_sha: str):
        return self.mirror.get(forge_sha)

    def rerun(self, number: int) -> None:
        self.reruns.append(number)

    def run_ci(self) -> None:
        raise AssertionError("external import started CI without an owner action")


def repository(case: bridge.Case) -> tuple[pathlib.Path, str]:
    repo = test_bridge.fixture_repo(pathlib.Path(tempfile.mkdtemp(prefix="intake-journey-git-")), {"README": b"Hello World!\n"})
    base = test_bridge.git_in(repo, "rev-parse", "HEAD").decode().strip()
    case.forge_base = base
    case.current_main = base
    case.apply_check = lambda patch, _base: subprocess.run(["git", "-C", str(repo), "apply", "--check", "--whitespace=error", "-"], input=patch, capture_output=True, check=False).stderr.decode() or None
    return repo, base


def commit_file(repo: pathlib.Path, text: str) -> str:
    (repo / "README").write_text(text)
    subprocess.run(["git", "-C", str(repo), "commit", "-qam", "move"], check=True, capture_output=True, env=dict(os.environ, GIT_COMMITTER_DATE="2026-10-28T00:00:00Z", GIT_AUTHOR_DATE="2026-10-28T00:00:00Z"))
    return subprocess.check_output(["git", "-C", str(repo), "rev-parse", "HEAD"], text=True).strip()


def stage_three(case: bridge.Case, repo: pathlib.Path, transport: FakeTransport) -> str:
    number = 42
    assert transport.labels(number) == ["external", "autofix:hold"]
    assert bridge.admit_run_ci(transport, number) == bridge.ALLOW
    assert transport.labels(number) == ["external", "autofix:hold", "run-ci"]
    assert transport.reruns == [number]
    assert not any("DELETE" in call[0] for call in transport.forge_calls)

    hostile = {
        "a non-owner actor": ({"login": "mallory"}, "owner"),
        "a forge pull request without external": ({"labels": []}, "hold"),
        "a merged pull request": ({"merged": True}, "hold"),
        "a missing audit comment": ({"audit": False}, "hold"),
        "a head whose diff moved": ({"head": "moved"}, "hold"),
        "a head whose workflows moved": ({"workflow_same": False}, "hold"),
    }
    for name, (bad, code) in hostile.items():
        fresh = FakeTransport(case, repo)
        fresh.imported = transport.imported
        fresh.mirror = dict(transport.mirror)
        pull = {"number": number, "head": {"ref": bridge.branch_name(case), "sha": transport.imported}, "labels": [{"name": "external"}], "state": "open", "merged": False}
        fresh.pulls[number] = pull
        audit = bridge.audit_payload(case, transport.imported, bridge.run_checks(case), "jack", None)
        fresh.comments[number] = [{"body": bridge.AUDIT_MARK + "\n```json\n" + json.dumps(audit) + "\n```"}] if bad.get("audit", True) else []
        fresh.login = bad.get("login", "jack")
        if "labels" in bad:
            pull["labels"] = [{"name": name} for name in bad["labels"]]
        if "merged" in bad:
            pull["merged"] = bad["merged"]
        if "head" in bad:
            pull["head"]["sha"] = commit_file(repo, "moved after the audit\n")
        if "workflow_same" in bad:
            fresh.workflow_same = bad["workflow_same"]
        decision = bridge.admit_run_ci(fresh, number)
        assert isinstance(decision, bridge.Refusal) and decision.code == code, f"{name}: {decision}"
        assert not any(call[1].endswith("/labels") for call in fresh.forge_calls), name
        assert not fresh.reruns, name
    return number


def stage_four(case: bridge.Case, transport: FakeTransport, number: int) -> str:
    landed = "f" * 40
    transport.mirror[transport.imported] = landed
    decision = bridge.land_case(transport, case)
    assert isinstance(decision, bridge.Refusal) and decision.code == "land", f"not merged: {decision}"
    assert not any("PATCH" in call[0] for call in transport.github_writes)
    assert not any(call[1].endswith("/issues/1/comments") for call in transport.github_writes)

    transport.pulls[number]["merged"] = True
    mirrorless = "e" * 40
    transport.pulls[number]["head"]["sha"] = mirrorless
    decision = bridge.land_case(transport, case)
    assert isinstance(decision, bridge.Refusal) and decision.code == "mapping", f"no mapping: {decision}"
    transport.pulls[number]["head"]["sha"] = transport.imported
    assert bridge.land_case(transport, case) == bridge.ALLOW
    comment = next(body for method, path, body in transport.github_writes if path.endswith(f"/issues/{case.number}/comments") and (body["body"] or "").startswith("landed as "))["body"]
    assert comment.startswith(f"landed as {landed} on Forgejo\n")
    assert bridge.REBASE in comment and comment.count(bridge.REBASE) == 1
    assert any(method == "PATCH" and path.endswith(f"/pulls/{case.number}") and body == {"state": "closed"} for method, path, body in transport.github_writes)

    missing = FakeTransport(case, transport.repo)
    missing.pulls[77] = {"number": 77, "head": {"ref": "gh/someone-else", "sha": "0" * 40}, "labels": [], "state": "open", "merged": True}
    decision = bridge.land_case(missing, case)
    assert isinstance(decision, bridge.Refusal) and decision.code == "land", f"no forge pull: {decision}"
    assert not missing.github_writes
    return landed


def mapped_import() -> str:
    case = test_bridge.mapped_case()
    repo, _ = repository(case)
    transport = FakeTransport(case, repo)
    with test_bridge.second_contributor():
        result = bridge.import_case(case, transport)
    assert result.decision == bridge.ALLOW, result.decision
    history = "".join(subprocess.check_output(["git", "-C", str(repo), "cat-file", "-p", sha]).decode() for sha in subprocess.check_output(["git", "-C", str(repo), "rev-list", "--all"], text=True).split())
    assert case.login not in history and case.contributor["emails"][0] not in history, "an imported commit object names the source account"
    comment = next(call[2]["body"] for call in transport.forge_calls if call[1].endswith("/issues/42/comments") and bridge.AUDIT_MARK in call[2]["body"])
    audit = json.loads(comment.split("```json", 1)[1].split("```", 1)[0])
    assert audit["github_login"] == case.login and audit["github_email"] == case.contributor["emails"][0], "the record lost the source account"
    assert audit["github_url"] == f"https://github.com/{bridge.GITHUB_REPO}/pull/{case.number}"
    assert audit["imported"] == transport.imported
    assert subprocess.check_output(["git", "-C", str(repo), "show", "-s", "--format=%an <%ae>", transport.imported], text=True).strip() == "Maximillian Ludwick <30126044+Madmaxme@users.noreply.github.com>"
    return transport.imported

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

    number = stage_three(case, repo, transport)
    landed = stage_four(case, transport, number)
    mapped_import()

    overridden = dataclasses.replace(case, commits=test_bridge.many_commits(case, case.contributor["caps"]["commits"] + 1))
    second_repo, _ = repository(overridden)
    second = FakeTransport(overridden, second_repo)
    override = {"check": "commits", "sha": overridden.head, "reason": "owner reviewed the extra commit"}
    refused_overrides = {
        "a non-owner actor": ("mallory", override),
        "a sha that is not the head": ("jack", {**override, "sha": "0" * 40}),
        "a blank reason": ("jack", {**override, "reason": ""}),
        "a check no override may waive": ("jack", {**override, "check": "path"}),
    }
    for name, (login, attempt) in refused_overrides.items():
        guarded = FakeTransport(overridden, second_repo)
        guarded.login = login
        result = bridge.import_case(overridden, guarded, attempt)
        assert isinstance(result.decision, bridge.Refusal) and result.decision.code == "override", f"{name}: {result.decision}"
        assert not guarded.pushes and not guarded.github_writes and not guarded.forge_calls, name
    accepted = bridge.import_case(overridden, second, override)
    assert accepted.decision == bridge.ALLOW
    assert accepted.audit["override"] == override
    assert accepted.audit["original_refusal"] == "intake: commits: 21 exceeds per-login cap 20"
    print(line)
    print(f"ALLOW {expected_branch} draft=42 labels={','.join(labels)} run-ci=held strict-base={base}")
    print("ok   intake stage-3 hold (owner-only run-ci, audited diff and workflows pinned)")
    print(f"ok   intake stage-4 landing (landed as {landed} on Forgejo, closed not merged, mapping from the commit map)")
    print("ok   intake import_as (the imported commit names the chosen identity, the forge record keeps the GitHub account)")
    print("ok   intake journey (real git, fake HTTP, zero network)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
