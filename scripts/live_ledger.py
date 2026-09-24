#!/usr/bin/env python3
"""The live lane's memory: run records on the CI-owned `telemetry` branch, a baseline read from
the last ten of the same model and routing, and a verdict naming every band and ratchet it breaks.

    live_ledger.py baseline RUN.json           -> baseline JSON of the runs like RUN (empty if none)
    live_ledger.py judge RUN.json BASELINE.json [AGAIN.json ...] -> verdict JSON; exit 1 when red
    live_ledger.py again BASELINE.json RUN.json [AGAIN.json ...] -> exit 0 when another run is due
    live_ledger.py append RUN.json SHA          -> commits runs/<utc>-<sha7>.json to `telemetry`

Bands are absolute (plan 2026-09-05 §3.E); ratchets compare with the baseline's medians, and
one holds only when every run of the head crossed it: the lane runs again, up to RUNS, while one does.
Only the verdict is deterministic law here; whether it blocks is the workflow's call (D133).
"""
import json, pathlib, statistics, subprocess, sys, tempfile, time

BRANCH = "telemetry"
LAST = 10
BANDS = {"ttftP50Ms": 3000, "toolSuccessMin": 0.95}
RATCHETS = {"ttftP50Ms": 0.25, "warmHitDrop": 0.10, "costPerScenario": 0.30}
RUNS = 3


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


def warm_rate(record):
    """The share of `cache-warm`'s warm-turn prompt read from the cache. Invariant: the whole-run
    rate moves with how many requests the model takes; this one has fixed turns and a settle."""
    for row in record.get("rows") or []:
        if row.get("task") == "cache-warm" and row.get("warmInput") is not None:
            total = (row.get("warmRead") or 0) + row["warmInput"]
            return row.get("warmRead", 0) / total if total else None
    return None


def measures(record):
    """The numbers a ratchet reads from one run; None where the run has none."""
    rows = record.get("rows") or []
    return {"ttftP50Ms": (record.get("telemetry") or {}).get("ttftP50Ms"), "warmHitRate": warm_rate(record),
            "costPerScenario": (record.get("spentUsd") or 0) / len(rows) if rows else None}


def baseline_of(records):
    """Medians, and each measure's worst run: a ratchet also has to clear the band the history spans."""
    runs = [r for r in records if not r.get("skipped")]
    classes = set()
    for r in runs:
        classes.update(((r.get("telemetry") or {}).get("classes") or {}).keys())
        for row in r.get("rows") or []:
            classes.update(row.get("classes") or [])
    baseline = {"runs": len(runs), "classes": sorted(classes)}
    for key in ("ttftP50Ms", "warmHitRate", "costPerScenario"):
        values = [m[key] for m in map(measures, runs) if m[key] is not None]
        baseline[key] = statistics.median(values) if values else None
        baseline[key + "Worst"] = (min if key == "warmHitRate" else max)(values) if values else None
    return baseline


def ratchets(record, baseline):
    """Each ratchet this one run crosses, by key; a verdict keeps those every run crossed. A bound is
    the median's ratchet or the history's worst run, whichever is further: a run inside the band is not news."""
    out, run = {}, measures(record)
    base, ttft = baseline.get("ttftP50Ms"), run["ttftP50Ms"]
    if base and ttft is not None and ttft > max(base * (1 + RATCHETS["ttftP50Ms"]), baseline.get("ttftP50MsWorst") or 0):
        out["ttftP50Ms"] = f"ratchet: ttft p50 {ttft} ms is +{(ttft / base - 1):.0%} over the baseline {base:.0f} ms"
    base, rate = baseline.get("warmHitRate"), run["warmHitRate"]
    worst = baseline.get("warmHitRateWorst")
    if base is not None and rate is not None and rate < min(base - RATCHETS["warmHitDrop"], 1 if worst is None else worst):
        out["warmHitRate"] = f"ratchet: warm-turn cache hit {rate:.0%} is {(base - rate):.0%} points under the baseline {base:.0%}"
    base, per = baseline.get("costPerScenario"), run["costPerScenario"]
    if base and per is not None and per > max(base * (1 + RATCHETS["costPerScenario"]), baseline.get("costPerScenarioWorst") or 0):
        out["costPerScenario"] = f"ratchet: cost per scenario ${per:.4f} is +{(per / base - 1):.0%} over the baseline ${base:.4f}"
    return out


def judge(record, baseline, again=()):
    """Every finding names the number, the bound and the source; red means at least one. Bands,
    classes and failures are the first run's; `again` are the head's later runs, for ratchets."""
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
        # A run with no number for a ratchet abstains: a missing measure is not a run that held.
        runs = [run for run in (record, *again) if not run.get("skipped")]
        crossed = [(ratchets(run, baseline), measures(run)) for run in runs]
        for key, finding in crossed[0][0].items():
            voted = [key in found for found, measured in crossed if measured[key] is not None]
            if all(voted):
                findings.append(finding + (f" in {len(voted)} of {len(voted)} runs" if len(voted) > 1 else ""))
    fails = [row["task"] for row in rows if row.get("status") == "fail"]
    if fails:
        findings.append("scenario(s) failed: " + ", ".join(fails))
    return {"red": bool(findings), "findings": findings}


def again(baseline, runs):
    """Whether the head is owed another run: a ratchet every run so far crossed, runs to spare."""
    return len(runs) < RUNS and bool(baseline.get("runs")) and any(
        "ratchet:" in f for f in judge(runs[0], baseline, runs[1:])["findings"])


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
    base = {"runs": 5, "ttftP50Ms": 800, "warmHitRate": 0.98, "costPerScenario": 0.01, "classes": ["tool"]}
    warm = {"task": "cache-warm", "status": "pass", "warmRead": 25600, "warmInput": 600}
    clean["rows"].append(warm)
    drift = json.loads(json.dumps(clean)); drift["telemetry"]["ttftP50Ms"] = 1100
    drift["rows"][1].update(warmRead=0, warmInput=26200)
    findings = judge(drift, base)["findings"]
    assert any("ratchet: ttft" in f for f in findings) and any("ratchet: warm-turn" in f for f in findings), findings
    assert warm_rate(clean) > 0.97 and warm_rate({"rows": [{"task": "cache-warm"}]}) is None
    busy = json.loads(json.dumps(clean)); busy["telemetry"]["hitRate"] = 0.1
    assert judge(busy, base)["findings"] == [], "more cold requests is the model's mix, not the cache"
    assert judge(drift, base, [clean, drift])["findings"] == [], "a ratchet one run held is noise"
    thrice = judge(drift, base, [drift, drift])["findings"]
    assert sum("in 3 of 3 runs" in f for f in thrice) == 2, thrice
    assert again(base, [drift]) and not again(base, [drift, clean]) and not again(base, [drift] * RUNS)
    blind = json.loads(json.dumps(drift)); blind["telemetry"] = {}; blind["rows"] = blind["rows"][:1]
    mute = judge(drift, base, [blind, drift])["findings"]
    assert sum("in 2 of 2 runs" in f for f in mute) == 2, f"a run with no number is no vote: {mute}"
    banded = judge(drift, {**base, "ttftP50MsWorst": 1200, "warmHitRateWorst": 0.0})["findings"]
    assert not any(f.startswith("ratchet:") for f in banded), "a run inside the history's worst is not news"
    novel = json.loads(json.dumps(clean)); novel["rows"][0]["classes"] = ["Invariant::lanes"]
    assert any("new error class" in f for f in judge(novel, base)["findings"])
    assert not any("new error class" in f for f in judge(novel, {})["findings"]), "no history, no novelty"
    failed = json.loads(json.dumps(clean)); failed["rows"][0]["status"] = "fail"
    assert judge(failed, {})["red"]
    skipped = judge({"skipped": "no key"}, base)
    assert not skipped["red"] and skipped["findings"] == ["inconclusive: no key"]
    b = baseline_of([clean, slow, {"skipped": "x"}])
    assert b["runs"] == 2 and b["ttftP50Ms"] == 2150 and b["classes"] == [] and b["warmHitRate"] > 0.97
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
    read = lambda path: json.loads(pathlib.Path(path).read_text()) if pathlib.Path(path).is_file() else {}
    if verb == "judge" and len(rest) >= 2:
        # An unmatched `live-again-*` glob arrives literally: a run that never happened is no run.
        verdict = judge(read(rest[0]), read(rest[1]), [read(path) for path in rest[2:] if pathlib.Path(path).is_file()])
        print(json.dumps(verdict, sort_keys=True)); return 1 if verdict["red"] else 0
    if verb == "again" and len(rest) >= 2:
        return 0 if again(read(rest[0]), [read(path) for path in rest[1:]]) else 1
    if verb == "append" and len(rest) == 2:
        return append(rest[0], rest[1])
    print(__doc__, file=sys.stderr); return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
