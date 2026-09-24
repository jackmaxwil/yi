#!/usr/bin/env python3
"""The live lane's memory: run records on the CI-owned `telemetry` branch, a baseline read from
the last ten of the same model and routing, and a verdict naming every band and ratchet it breaks.

    live_ledger.py baseline RUN.json           -> baseline JSON of the runs like RUN (empty if none)
    live_ledger.py judge RUN.json BASELINE.json -> verdict JSON on stdout; exit 1 when red
    live_ledger.py append RUN.json SHA          -> commits runs/<utc>-<sha7>.json to `telemetry`

Bands are absolute (plan 2026-09-05 §3.E); ratchets compare with the baseline's medians.
Only the verdict is deterministic law here; whether it blocks is the workflow's call (D133).
"""
import json, pathlib, statistics, subprocess, sys, tempfile, time

BRANCH = "telemetry"
LAST = 10
BANDS = {"ttftP50Ms": 3000, "toolSuccessMin": 0.95}
RATCHETS = {"ttftP50Ms": 0.25, "hitRateDrop": 0.10, "costPerScenario": 0.30}


import os

GIT_ENV = {**os.environ, "GIT_TERMINAL_PROMPT": "0"}


def git(*args, check=True):
    """No prompt and no patience: a credential question on a runner is a hang."""
    return subprocess.run(["git", *args], capture_output=True, text=True, check=check, env=GIT_ENV, timeout=120).stdout


def kind(record):
    """What a ratchet may compare across: the model and the routing. A record from before `mode`
    was written ran on OpenRouter's default routing, which is plain `live`."""
    return record.get("model"), record.get("mode", "live")


def history(like):
    """The last records on the telemetry branch that ran like `like`, newest first; none is not
    an error. A new routing starts its own history rather than inheriting another's medians."""
    try:
        git("fetch", "--no-tags", "origin", f"{BRANCH}:refs/remotes/origin/{BRANCH}")
        names = sorted(git("ls-tree", "--name-only", f"origin/{BRANCH}", "runs/").split())
    except subprocess.CalledProcessError:
        return []
    records = []
    for name in reversed(names):
        if len(records) == LAST:
            break
        try:
            record = json.loads(git("show", f"origin/{BRANCH}:{name}"))
        except (subprocess.CalledProcessError, json.JSONDecodeError):
            continue
        if kind(record) == kind(like):
            records.append(record)
    return records


def baseline_of(records):
    def med(values):
        values = [v for v in values if v is not None]
        return statistics.median(values) if values else None
    tele = [r.get("telemetry") or {} for r in records if not r.get("skipped")]
    scenarios = [len(r.get("rows") or []) for r in records if not r.get("skipped")]
    classes = set()
    for t in tele:
        classes.update((t.get("classes") or {}).keys())
    for r in records:
        for row in r.get("rows") or []:
            classes.update(row.get("classes") or [])
    return {
        "runs": len(tele),
        "ttftP50Ms": med([t.get("ttftP50Ms") for t in tele]),
        "hitRate": med([t.get("hitRate") for t in tele]),
        "costPerScenario": med([
            (r.get("spentUsd") or 0) / n for r, n in zip([x for x in records if not x.get("skipped")], scenarios) if n
        ]),
        "classes": sorted(classes),
    }


def judge(record, baseline):
    """Every finding names the number, the bound and the source; red means at least one."""
    findings = []
    if record.get("skipped"):
        return {"red": False, "findings": [f"inconclusive: {record['skipped']}"]}
    tele = record.get("telemetry") or {}
    rows = record.get("rows") or []
    ttft = tele.get("ttftP50Ms")
    if ttft is not None and ttft > BANDS["ttftP50Ms"]:
        findings.append(f"band: ttft p50 {ttft} ms > {BANDS['ttftP50Ms']} ms")
    tools = tele.get("tools") or []
    calls = sum(t.get("calls", 0) for t in tools)
    errors = sum(t.get("errors", 0) for t in tools)
    if calls and (calls - errors) / calls < BANDS["toolSuccessMin"]:
        findings.append(f"band: tool success {(calls - errors) / calls:.0%} < {BANDS['toolSuccessMin']:.0%}")
    if record.get("budgetHit"):
        findings.append(f"band: spent ${record.get('spentUsd', 0):.4f} hit the cap ${record.get('capUsd', 0):.2f}")
    seen = set((tele.get("classes") or {}).keys())
    for row in rows:
        seen.update(row.get("classes") or [])
    new = sorted(seen - set(baseline.get("classes") or []))
    if new and baseline.get("runs"):
        findings.append("new error class(es) absent from the last runs: " + ", ".join(new))
    if baseline.get("runs"):
        base = baseline.get("ttftP50Ms")
        if base and ttft is not None and ttft > base * (1 + RATCHETS["ttftP50Ms"]):
            findings.append(f"ratchet: ttft p50 {ttft} ms is +{(ttft / base - 1):.0%} over the baseline {base:.0f} ms")
        base = baseline.get("hitRate")
        rate = tele.get("hitRate")
        if base is not None and rate is not None and rate < base - RATCHETS["hitRateDrop"]:
            findings.append(f"ratchet: cache hit {rate:.0%} is {(base - rate):.0%} points under the baseline {base:.0%}")
        base = baseline.get("costPerScenario")
        if base and rows:
            per = (record.get("spentUsd") or 0) / len(rows)
            if per > base * (1 + RATCHETS["costPerScenario"]):
                findings.append(f"ratchet: cost per scenario ${per:.4f} is +{(per / base - 1):.0%} over the baseline ${base:.4f}")
    fails = [row["task"] for row in rows if row.get("status") == "fail"]
    if fails:
        findings.append("scenario(s) failed: " + ", ".join(fails))
    return {"red": bool(findings), "findings": findings}


def append(run_path, sha):
    record = json.loads(pathlib.Path(run_path).read_text())
    name = f"runs/{time.strftime('%Y%m%dT%H%M%SZ', time.gmtime())}-{sha[:7]}.json"
    with tempfile.TemporaryDirectory(prefix="yi-telemetry-") as tmp:
        exists = subprocess.run(["git", "fetch", "--no-tags", "origin", f"{BRANCH}:refs/remotes/origin/{BRANCH}"],
                                capture_output=True, env=GIT_ENV, timeout=120).returncode == 0
        if exists:
            git("worktree", "add", "-q", "--detach", tmp, f"origin/{BRANCH}")
        else:
            git("worktree", "add", "-q", "--detach", tmp)
            subprocess.run(["git", "-C", tmp, "checkout", "-q", "--orphan", BRANCH], check=True)
            subprocess.run(["git", "-C", tmp, "rm", "-rfq", "."], check=False)
        target = pathlib.Path(tmp) / name
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(json.dumps(record, indent=1, sort_keys=True))
        subprocess.run(["git", "-C", tmp, "add", name], check=True)
        subprocess.run(["git", "-C", tmp, "-c", "user.name=yi-ci", "-c", "user.email=ci@yi", "commit", "-q", "-m", f"live: {name}"], check=True)
        pushed = subprocess.run(["git", "-C", tmp, "push", "-q", "origin", f"HEAD:refs/heads/{BRANCH}"], capture_output=True, text=True, env=GIT_ENV, timeout=120)
        git("worktree", "remove", "--force", tmp, check=False)
    print(name, "pushed" if pushed.returncode == 0 else f"not pushed: {pushed.stderr.strip()[:200]}")
    return 0


def selfcheck():
    clean = {"rows": [{"task": "a", "status": "pass", "classes": []}], "spentUsd": 0.01, "capUsd": 1,
             "telemetry": {"ttftP50Ms": 800, "hitRate": 0.5, "tools": [{"calls": 4, "errors": 0}], "classes": {}}}
    assert judge(clean, {}) == {"red": False, "findings": []}
    slow = json.loads(json.dumps(clean)); slow["telemetry"]["ttftP50Ms"] = 3500
    assert any(f.startswith("band: ttft") for f in judge(slow, {})["findings"])
    base = {"runs": 5, "ttftP50Ms": 800, "hitRate": 0.5, "costPerScenario": 0.01, "classes": ["tool"]}
    drift = json.loads(json.dumps(clean)); drift["telemetry"]["ttftP50Ms"] = 1100; drift["telemetry"]["hitRate"] = 0.3
    findings = judge(drift, base)["findings"]
    assert any("ratchet: ttft" in f for f in findings) and any("ratchet: cache hit" in f for f in findings), findings
    novel = json.loads(json.dumps(clean)); novel["rows"][0]["classes"] = ["Invariant::lanes"]
    assert any("new error class" in f for f in judge(novel, base)["findings"])
    assert not any("new error class" in f for f in judge(novel, {})["findings"]), "no history, no novelty"
    failed = json.loads(json.dumps(clean)); failed["rows"][0]["status"] = "fail"
    assert judge(failed, {})["red"]
    skipped = judge({"skipped": "no key"}, base)
    assert not skipped["red"] and skipped["findings"] == ["inconclusive: no key"]
    b = baseline_of([clean, slow, {"skipped": "x"}])
    assert b["runs"] == 2 and b["ttftP50Ms"] == 2150 and b["classes"] == []
    pinned = {**clean, "model": "m", "mode": 'live+routing{"order":["x"]}'}
    assert kind({"model": "m"}) == kind({"model": "m", "mode": "live"}), "an old record ran plain live"
    assert kind(pinned) != kind({"model": "m"}), "a pinned run is never judged by unpinned medians"
    print("ok   live_ledger selfcheck")


def main(argv):
    if not argv or argv[0] == "--selfcheck":
        selfcheck(); return 0
    verb, rest = argv[0], argv[1:]
    if verb == "baseline" and len(rest) == 1:
        like = json.loads(pathlib.Path(rest[0]).read_text())
        print(json.dumps(baseline_of(history(like)), sort_keys=True)); return 0
    if verb == "judge" and len(rest) == 2:
        record = json.loads(pathlib.Path(rest[0]).read_text())
        baseline = json.loads(pathlib.Path(rest[1]).read_text()) if pathlib.Path(rest[1]).is_file() else {}
        verdict = judge(record, baseline)
        print(json.dumps(verdict, sort_keys=True)); return 1 if verdict["red"] else 0
    if verb == "append" and len(rest) == 2:
        return append(rest[0], rest[1])
    print(__doc__, file=sys.stderr); return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
