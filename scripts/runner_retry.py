#!/usr/bin/env python3
"""Classify failed gate logs and rerun only runs the runner broke."""
import argparse
import contextlib
import io
import pathlib
import re
import subprocess
import sys
from unittest import mock

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "scripts"))
import forge_pr
import forge_tracking

OPS = pathlib.Path.home() / "Development/buildhost/the infra repository-apply"
MAX_RETRIES = 2
CODE_FAILURE = re.compile(
    r"^(?:\S+ )?error(?:\[[^\]]+\])?: (?!recipe `|test run failed)"
    r"|Summary \[[^\n]*[1-9]\d* failed|^\S+ FAIL \w+",
    re.M,
)
RUNNER_FAILURES = (
    (
        "docker-daemon",
        re.compile(r"Cannot connect to the Docker daemon", re.I),
        'failed to inspect exec: Cannot connect to the Docker daemon at tcp://buildhost-vm123-e-dind:2375. '
        'Is the docker daemon running?',
    ),
    (
        "copy-deadline",
        re.compile(r"unable to copy [^\n]*context deadline exceeded|⚙️ \[runner\]: context deadline exceeded", re.I),
        '2026-11-03T19:55:57.4013978Z     error: WaitFrame(5000, true, "s-new") timed out\n'
        '2026-11-03T20:48:30.1150487Z Summary [407.249s] 4107 tests run: 4107 passed (1 slow), 63 skipped\n'
        '⚙️ [runner]: context deadline exceeded\\nunable to copy '
        '"/var/run/act/workflow/SUMMARY.md" from container to host: context deadline exceeded',
    ),
    (
        "missing-image",
        re.compile(r"No such image", re.I),
        "Error response from daemon: No such image: yi-term:dev",
    ),
    (
        "dns",
        re.compile(r"Could not resolve host", re.I),
        "⚙️ [runner]: could not fetch remote 'origin': fatal: unable to access "
        "'https://github.com/actions/checkout/': Could not resolve host: github.com "
        "(DNS server returned general failure): exit status 128",
    ),
    (
        "disk-full",
        re.compile(r"No space left on device|ENOSPC|Out of diskspace", re.I),
        "failed to create container: write /var/lib/docker/tmp: ENOSPC: no space left on device",
    ),
)
CODE_LOGS = (
    (
        "clippy",
        "cargo clippy --workspace --all-targets -- -D warnings\n"
        "2026-11-03T18:39:54.7601752Z error: unused imports: `PathBuf` and `Path`\n"
        "2026-11-03T18:39:59.8217574Z error: could not compile `yi-tools` (lib test) due to 1 previous error",
    ),
    (
        "nextest",
        "2026-11-03T20:35:56.2007212Z Summary [92.772s] 736/4107 tests run: "
        "735 passed (1 slow), 1 failed, 63 skipped\n"
        "2026-11-03T20:35:56.2022493Z error: test run failed",
    ),
)


class RetryError(RuntimeError):
    pass


def classify_log(text, off=""):
    if CODE_FAILURE.search(text):
        return "code", "compiler or test failure"
    for name, pattern, _ in RUNNER_FAILURES:
        if name != off and pattern.search(text):
            return "runner", name
    return "code", "no runner failure signature"


def failed_tasks(name, sha, jobs):
    wanted = set(jobs)
    found = {}

    def transport(method, path, payload):
        answer = forge_pr.fgj_api(method, path, payload)
        rows = answer.get("workflow_runs") if isinstance(answer, dict) else None
        if rows is None:
            raise RetryError(f"the forge did not list failed tasks: {answer}")
        for row in rows:
            job = row.get("name")
            if row.get("head_sha") == sha and job in wanted and job not in found:
                found[job] = row
        return [] if wanted == found.keys() else rows

    list(forge_tracking.paged(transport, f"repos/{name}/actions/tasks?status=failure"))
    missing = wanted - found.keys()
    if missing:
        raise RetryError(f"no failed task on {sha[:12]} for {', '.join(sorted(missing))}")
    return [found[job] for job in jobs]


def read_log(task):
    out = subprocess.run(
        ("just", "ci-log", f"task:{task['id']}", "0"),
        cwd=OPS,
        capture_output=True,
        text=True,
        errors="replace",
        timeout=90,
        check=False,
    )
    if out.returncode:
        lines = out.stderr.strip().splitlines()
        detail = lines[-1] if lines else f"exit {out.returncode}"
        raise RetryError(f"task {task['id']} log: {detail}")
    return out.stdout


RETRY_ROW = re.compile(r"<!-- yi-runner-retry sha=([0-9a-f]{40}) attempt=(\d+) -->")


def comments(name, number):
    def transport(method, path, payload):
        answer = forge_pr.fgj_api(method, path, payload)
        if not isinstance(answer, list):
            raise RetryError(f"the forge did not list PR comments: {answer}")
        return answer

    return list(forge_tracking.paged(transport, f"repos/{name}/issues/{number}/comments?x=1"))


def retries_of(notes, sha):
    attempts = {int(match.group(2)) for note in notes
                if (match := RETRY_ROW.search(note.get("body") or "")) and match.group(1) == sha}
    return max(attempts, default=0)


def owns_attempt(notes, sha, attempt, comment_id):
    """Invariant: the lowest comment id for one deterministic attempt reruns; later duplicates abstain."""
    marker = f"<!-- yi-runner-retry sha={sha} attempt={attempt} -->"
    owners = [note["id"] for note in notes if marker in (note.get("body") or "")]
    return bool(owners) and comment_id == min(owners)


def workflow_run_id(name, run_number, sha):
    answer = forge_pr.fgj_api("GET", f"repos/{name}/actions/runs?run_number={run_number}")
    rows = answer.get("workflow_runs") if isinstance(answer, dict) else None
    matches = [row for row in rows or []
               if row.get("index_in_repo") == run_number and row.get("commit_sha") == sha]
    if len(matches) != 1 or not isinstance(matches[0].get("id"), int):
        raise RetryError(f"run sequence {run_number} did not resolve to one run on {sha[:12]}")
    return matches[0]["id"]


def rerun(run_id):
    return subprocess.run(
        ("fgj", "actions", "run", "rerun", str(run_id), *forge_pr.scope()),
        capture_output=True,
        text=True,
        errors="replace",
        check=False,
    )


def retry(args):
    number = forge_pr.pull_number(args.number)
    pr = forge_pr.pull(number)
    waiting = forge_pr.rerun_waits(pr)
    if waiting:
        raise RetryError(f"a run is already waiting on {', '.join(waiting)}")
    jobs = forge_pr.jobs_of(pr)
    failed = [job for job in forge_pr.required() if jobs.get(job) == "failure"]
    if not failed:
        print(f"#{number}: no failed gate jobs on {pr['head']['sha'][:12]}")
        return 0
    sha = pr["head"]["sha"]
    classified = []
    for task in failed_tasks(forge_pr.repo(), sha, failed):
        kind, reason = classify_log(read_log(task))
        print(f"#{number}: {task['name']} task {task['id']}: {kind} ({reason})")
        classified.append((task, kind, reason))
    candidates = {}
    code_runs = set()
    for task, kind, reason in classified:
        run_id = task.get("run_number")
        if run_id is None:
            raise RetryError(f"task {task['id']} has no run id")
        if kind == "runner":
            candidates.setdefault(run_id, []).append(reason)
        else:
            code_runs.add(run_id)
    selected = [run_id for run_id in candidates if run_id not in code_runs]
    for run_id in candidates.keys() & code_runs:
        print(f"#{number}: run {run_id} also has a code failure; not rerun")
    if not selected:
        print(f"#{number}: no runner-only failed run to retry")
        return 1
    run_ids = [workflow_run_id(forge_pr.repo(), run_number, sha) for run_number in selected]
    notes = comments(forge_pr.repo(), number)
    used = retries_of(notes, sha)
    if used >= MAX_RETRIES:
        print(f"#{number}: {MAX_RETRIES} runner retries already spent on {sha[:12]}")
        return 1
    attempt = used + 1
    reasons = sorted({reason for run_id in selected for reason in candidates[run_id]})
    body = (
        f"<!-- yi-runner-retry sha={sha} attempt={attempt} -->\n"
        f"Runner retry {attempt} of {MAX_RETRIES} for run(s) "
        f"{', '.join(str(run_id) for run_id in run_ids)}: {', '.join(reasons)}."
    )
    answer = forge_pr.fgj_api("POST", f"repos/{forge_pr.repo()}/issues/{number}/comments", {"body": body})
    if not isinstance(answer, dict) or answer.get("message") or "id" not in answer:
        raise RetryError(f"the retry ledger was not written: {answer}")
    if not owns_attempt(comments(forge_pr.repo(), number), sha, attempt, answer["id"]):
        print(f"#{number}: another invocation owns runner retry {attempt}; not rerunning")
        return 0
    ok = True
    for run_id in run_ids:
        out = rerun(run_id)
        if out.returncode:
            ok = False
            detail = (out.stderr or out.stdout).strip()
            print(f"#{number}: run {run_id} rerun refused: {detail}")
        else:
            print(f"#{number}: reran run {run_id} (runner retry {used + 1} of {MAX_RETRIES})")
    return 0 if ok else 1


def cmd_retry(args):
    try:
        return retry(args)
    except (RetryError, subprocess.TimeoutExpired) as err:
        print(f"retry: {err}")
        return 1


def classifier_cases(off=""):
    bad = []
    for name, _, sample in RUNNER_FAILURES:
        if classify_log(sample, off) != ("runner", name):
            bad.append(f"signature {name} did not classify its sample")
    for name, log in CODE_LOGS:
        if classify_log(log, off)[0] != "code":
            bad.append(f"real {name} failure classified as runner")
    if classify_log("unrecognized failure", off)[0] != "code":
        bad.append("an unknown failure classified as runner")
    if classify_log(CODE_LOGS[0][1] + "\nCannot connect to the Docker daemon", off)[0] != "code":
        bad.append("a runner signature overrode a code failure")
    return bad


def check_task_discovery():
    sha = "a" * 40
    rows = [
        {"id": 8, "name": "gate (test)", "head_sha": "b" * 40, "run_number": 3},
        {"id": 7, "name": "gate (lint)", "head_sha": sha, "run_number": 2},
        {"id": 6, "name": "gate (test)", "head_sha": sha, "run_number": 2},
        {"id": 5, "name": "gate (test)", "head_sha": sha, "run_number": 1},
    ]
    calls = []

    def api(method, path, payload=None):
        calls.append(path)
        return {"workflow_runs": rows[:2] if "page=1" in path else rows[2:]}

    with mock.patch.object(forge_pr, "fgj_api", api):
        found = failed_tasks("o/r", sha, ["gate (test)", "gate (lint)"])
    assert [row["id"] for row in found] == [6, 7], found
    assert len(calls) == 2, calls


def check_retry_flow():
    number = 7
    sha = "a" * 40
    pr = {"number": number, "head": {"sha": sha}}
    tasks = [
        {"id": 6, "name": "gate (test)", "run_number": 42},
        {"id": 7, "name": "gate (lint)", "run_number": 42},
    ]
    logs = {6: "Cannot connect to the Docker daemon", 7: "No such image: yi-term"}
    notes, posts, runs = [], [], []

    def api(method, path, payload=None):
        if method == "GET":
            assert "/comments" in path, path
            return notes if "page=1" in path else []
        assert method == "POST", (method, path)
        posts.append(payload)
        notes.append({"id": len(posts), "body": payload["body"]})
        return {"id": len(posts)}

    def run(run_id, **kwargs):
        runs.append(run_id)
        return subprocess.CompletedProcess((run_id,), 0, "", "")

    patches = (
        mock.patch.object(forge_pr, "pull_number", lambda value: number),
        mock.patch.object(forge_pr, "pull", lambda value: pr),
        mock.patch.object(forge_pr, "rerun_waits", lambda value: []),
        mock.patch.object(forge_pr, "jobs_of", lambda value: {"gate (test)": "failure", "gate (lint)": "failure"}),
        mock.patch.object(forge_pr, "required", lambda: ["gate (test)", "gate (lint)"]),
        mock.patch.object(forge_pr, "repo", lambda: "o/r"),
        mock.patch.object(forge_pr, "fgj_api", api),
        mock.patch.object(sys.modules[__name__], "failed_tasks", lambda repo, head, jobs: tasks),
        mock.patch.object(sys.modules[__name__], "read_log", lambda task: logs[task["id"]]),
        mock.patch.object(sys.modules[__name__], "workflow_run_id", lambda repo, number, sha: number + 17800),
        mock.patch.object(sys.modules[__name__], "rerun", run),
    )
    with contextlib.ExitStack() as stack:
        for patch in patches:
            stack.enter_context(patch)
        with contextlib.redirect_stdout(io.StringIO()):
            codes = [cmd_retry(argparse.Namespace(number=str(number))) for _ in range(3)]
            logs[6] = CODE_LOGS[0][1]
            code = cmd_retry(argparse.Namespace(number=str(number)))
        assert codes == [0, 0, 1], codes
        assert len(posts) == 2 and runs == [17842, 17842], (posts, runs)
        notes.clear()
        posts.clear()
        runs.clear()
        assert code == 1 and not posts and not runs, "a code failure was rerun"
        logs[6] = "Cannot connect to the Docker daemon"
        logs[7] = CODE_LOGS[0][1]
        with contextlib.redirect_stdout(io.StringIO()):
            mixed = cmd_retry(argparse.Namespace(number=str(number)))
        assert mixed == 1 and not posts and not runs, "a run mixing code and runner failures was rerun"
        tasks[1]["run_number"] = 43
        with contextlib.redirect_stdout(io.StringIO()):
            separate = cmd_retry(argparse.Namespace(number=str(number)))
        assert separate == 0 and runs == [17842] and len(posts) == 1, "a code-failed run rode a runner retry"


def check_ledger():
    sha = "a" * 40
    page1 = [{"id": n, "body": "other"} for n in range(50)]
    old = [
        {"id": 50, "body": f"<!-- yi-runner-retry sha={sha} attempt=1 -->"},
        {"id": 51, "body": f"<!-- yi-runner-retry sha={sha} attempt=2 -->"},
        {"id": 52, "body": f"<!-- yi-runner-retry sha={sha} attempt=2 -->"},
    ]

    def api(method, path, payload=None):
        return [*page1, *old] if "page=1" in path else []

    with mock.patch.object(forge_pr, "fgj_api", api):
        notes = comments("o/r", 7)
    assert len(notes) == 53, len(notes)
    assert retries_of(notes, sha) == 2, notes
    assert owns_attempt(notes, sha, 2, 51)
    assert not owns_attempt(notes, sha, 2, 52)



def check_workflow_run_id():
    sha = "a" * 40
    row = {"id": 17842, "index_in_repo": 13269, "commit_sha": sha}
    with mock.patch.object(forge_pr, "fgj_api", lambda *args: {"workflow_runs": [row]}):
        assert workflow_run_id("o/r", 13269, sha) == 17842
    with mock.patch.object(forge_pr, "fgj_api", lambda *args: {"workflow_runs": [row]}):
        try:
            workflow_run_id("o/r", 13269, "b" * 40)
        except RetryError:
            pass
        else:
            raise AssertionError("a sequence resolved on another head")


def check_rerun_command():
    calls = []

    def run(command, **kwargs):
        calls.append(command)
        return subprocess.CompletedProcess(command, 0, "", "")

    with mock.patch.object(forge_pr, "scope", lambda: ("-R", "o/r", "--hostname", "git.example.invalid")), \
            mock.patch.object(subprocess, "run", run):
        rerun(42)
    assert calls == [("fgj", "actions", "run", "rerun", "42", "-R", "o/r", "--hostname", "git.example.invalid")], calls


def check_log_command():
    calls = []

    def run(command, **kwargs):
        calls.append((command, kwargs))
        return subprocess.CompletedProcess(command, 0, "whole log", "")

    with mock.patch.object(subprocess, "run", run):
        assert read_log({"id": 6}) == "whole log"
    command, kwargs = calls[0]
    assert command == ("just", "ci-log", "task:6", "0"), command
    assert kwargs["cwd"] == pathlib.Path.home() / "Development/buildhost/the infra repository-apply", kwargs
    assert kwargs["capture_output"] and not kwargs["check"], kwargs


def check_status_hint():
    pr = {"number": 7, "title": "Keep gate", "state": "open", "head": {"sha": "a" * 40, "ref": "x"}}
    with contextlib.redirect_stdout(io.StringIO()) as said:
        forge_pr.report(pr, {"gate (test)": "failure"}, ["gate (test)"], "failed:gate (test)")
    assert "just pr retry 7" in said.getvalue(), said.getvalue()


def check_merge_flow():
    number = 7
    state = {"retried": False, "merged": False}
    pr = {"number": number, "title": "Keep the gate", "state": "open", "merged": False,
          "head": {"sha": "abc1234", "ref": "x"}, "base": {"ref": "main"}}

    def pull(value):
        return dict(pr, merged=state["merged"])

    def jobs(value):
        status = "success" if state["retried"] else "failure"
        return {"gate (test)": status}

    def api(method, path, payload=None):
        state["merged"] = True
        return {}

    def retried(args):
        state["retried"] = True
        return 0

    patches = (
        mock.patch.object(forge_pr, "pull", pull),
        mock.patch.object(forge_pr, "jobs_of", jobs),
        mock.patch.object(forge_pr, "required", lambda: ["gate (test)"]),
        mock.patch.object(forge_pr, "is_behind", lambda value: False),
        mock.patch.object(forge_pr, "unreviewed", lambda *args: []),
        mock.patch.object(forge_pr, "fgj_api", api),
        mock.patch.object(forge_pr, "cmd_retry", retried),
        mock.patch.object(forge_pr, "POLL", 0),
    )
    with contextlib.ExitStack() as stack:
        for patch in patches:
            stack.enter_context(patch)
        with contextlib.redirect_stdout(io.StringIO()):
            code = forge_pr.cmd_merge(argparse.Namespace(number=number, timeout=1, wait=True))
    assert code == 0 and state["retried"] and state["merged"], (code, state)


def selfcheck():
    bad = classifier_cases()
    for name, _, _ in RUNNER_FAILURES:
        if not any(name in reason for reason in classifier_cases(name)):
            bad.append(f"signature {name} disabled and every classifier case still passes")
    for name, check in (("task discovery", check_task_discovery), ("retry flow", check_retry_flow),
                        ("ledger", check_ledger), ("run id", check_workflow_run_id),
                        ("rerun command", check_rerun_command), ("log command", check_log_command),
                        ("status hint", check_status_hint), ("merge flow", check_merge_flow)):
        try:
            check()
        except AssertionError as err:
            bad.append(f"{name}: {err}")
    if bad:
        print("\n".join(f"FAIL runner retry selfcheck: {reason}" for reason in bad))
        return 1
    print("ok   runner retry selfcheck")
    return 0


def main(argv):
    if "--selfcheck" in argv:
        return selfcheck()
    parser = argparse.ArgumentParser(prog="just pr retry")
    parser.add_argument("number", nargs="?")
    args = parser.parse_args(argv)
    return cmd_retry(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
