#!/usr/bin/env python3
"""Five axes from any run directory (docs/plans/2026-09-06-tbv4-evals.md, S2; D140).

    python3 evals/axes.py <dir> [--suite S] [--model M] [--json out.jsonl]

A trial is a v4 session file; its context is the nearest ancestor holding a
harbor `result.json` (reward, wall, timeout; the verifier's `ctrf.json` and
`trace_results.json` beside it) or a run.py `row.json`. One JSON line per
trial, then the docs/eval-ledger.md row with the persistence, rigor and
experience triples, timeouts and partials on the right. Exit 2 when a trial
is unmeasurable: a turn without usage, or no assistant message at all. Every
column is a deterministic function of the run's own files (axes.md names each
source).
"""

import argparse
import json
import statistics
import sys
from datetime import datetime
from math import comb
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT.parent / "skills" / "yi" / "session-mining"))

import extract  # noqa: E402
import yi_usage  # noqa: E402

PASS_AT_KS = (2, 3, 4, 5)
CI95_Z = 1.96


def is_session(path):
    if path.name.endswith(".telemetry.jsonl"):
        return False
    try:
        with path.open(errors="replace") as handle:
            first = handle.readline()
        head = json.loads(first)
    except (OSError, ValueError):
        return False
    return isinstance(head, dict) and head.get("kind") == "header"


def context_of(path, root):
    for parent in path.parents:
        if (parent / "result.json").is_file():
            return "harbor", parent
        if (parent / "row.json").is_file():
            return "run", parent
        if parent == root:
            break
    return "journey", path.parent


def _iso_seconds(timing):
    try:
        start = datetime.fromisoformat(timing["started_at"].replace("Z", "+00:00"))
        end = datetime.fromisoformat(timing["finished_at"].replace("Z", "+00:00"))
    except (KeyError, TypeError, ValueError, AttributeError):
        return None
    return round((end - start).total_seconds(), 2)


def verifier_report(trial, name):
    """A file the task's verifier wrote beside its reward, or {} when this verifier writes none."""
    path = trial / "verifier" / name
    report = json.loads(path.read_text()) if path.is_file() else {}
    return report if isinstance(report, dict) else {}


def harbor_context(trial):
    result = json.loads((trial / "result.json").read_text())
    rewards = (result.get("verifier_result") or {}).get("rewards") or {}
    exc = (result.get("exception_info") or {}).get("exception_type") or ""
    reward = rewards.get("reward")
    tally = (verifier_report(trial, "ctrf.json").get("results") or {}).get("summary") or {}
    partial = verifier_report(trial, "trace_results.json").get("partial_score")
    return {
        "task": result.get("task_name"),
        "reward": float(reward) if isinstance(reward, (int, float)) else 0.0,
        "timedOut": exc == "AgentTimeoutError",
        "errored": exc not in ("", "AgentTimeoutError", "VerifierTimeoutError"),
        # Incident: photonic cc3HUqA's verifier overran its 300 s ceiling and harbor kept the agent
        # timeout over the VerifierTimeoutError; a verifier that started and returned nothing survives.
        "verifierUnmeasured": result.get("verifier_result") is None and bool(result.get("verifier")),
        "testsPassed": tally.get("passed"),
        "testsTotal": tally.get("tests"),
        "partialScore": partial if isinstance(partial, (int, float)) else None,
        "wallSec": _iso_seconds(result.get("agent_execution") or {}),
    }


def run_context(trial):
    row = json.loads((trial / "row.json").read_text())
    return {"task": row.get("task"), "reward": float(row.get("reward") or 0), "timedOut": bool(row.get("timedOut")),
            "errored": row.get("exit") not in (0, None), "wallSec": row.get("wallSec")}


def telemetry(path):
    sidecar = path.with_name(path.name + ".telemetry.jsonl")
    if not sidecar.is_file():
        sidecar = path.with_suffix(".telemetry.jsonl")
    spans, _ = yi_usage.json_lines(sidecar)
    ttft = [s["ttftMs"] for s in spans if s.get("span") == "request" and isinstance(s.get("ttftMs"), int)]
    tools = [s for s in spans if s.get("span") == "tool"]
    failed = sum(1 for s in tools if s.get("ok") is False)
    return {
        "ttftP50Ms": int(statistics.median(ttft)) if ttft else None,
        "toolErrorRate": round(failed / len(tools), 4) if tools else None,
    }


def unknown_turns(entries):
    count = 0
    for entry in entries:
        message = entry.get("message") or {}
        if entry.get("type") == "message" and message.get("role") == "assistant":
            if (message.get("usage") or {}).get("unknown") is True:
                count += 1
    return count


def score(path, root):
    header, entries, corrupt = extract.read_session(path)
    census = {name: extract.collections.Counter() for name in ("entry", "role", "tool", "error", "custom")}
    mu, _issues, _orientation, _delegation = extract.extract_session(path, header or {}, entries, census)
    kind, trial = context_of(path, root)
    context = {"harbor": harbor_context, "run": run_context}.get(kind, lambda t: {
        "task": t.name, "reward": None, "timedOut": False, "errored": False, "wallSec": None})(trial)
    tokens, signals = mu["tokens"], mu["signals"]
    prompt = tokens["input"] + tokens["cacheRead"]
    unknown = unknown_turns(entries)
    row = {
        "session": mu["sessionId"], "kind": kind, "trial": trial.name, **context,
        "turns": mu["turns"], "corruptLines": corrupt, "unknownTurns": unknown,
        "input": tokens["input"], "cacheRead": tokens["cacheRead"], "cacheWrite": tokens["cacheWrite"],
        "output": tokens["output"],
        "costUsd": None if unknown else tokens["costUsd"],
        "cacheHitRate": round(tokens["cacheRead"] / prompt, 4) if prompt else None,
        "tokensPerTurn": round(prompt / mu["turns"]) if mu["turns"] else None,
        "peakContextTokens": mu["peakContextTokens"], "compactions": mu["compactions"],
        "repeatedCalls": mu["repeatedCalls"], "interrupts": mu["friction"]["interrupts"],
        "readsBeforeMutation": mu["orientation"]["callsBeforeFirstMutation"],
        "signals": signals, **telemetry(path),
        "measurable": mu["turns"] > 0 and unknown == 0,
    }
    return row


def pass_at_k(by_task, ks=PASS_AT_KS):
    out = {}
    for k in ks:
        vals = []
        for rewards in by_task.values():
            n, c = len(rewards), sum(1 for r in rewards if r and r > 0)
            if n < k:
                continue
            vals.append(1.0 if n - c < k else 1.0 - comb(n - c, k) / comb(n, k))
        if vals:
            out[f"pass_at_{k}"] = round(sum(vals) / len(vals), 4)
    return out


def ci95(by_task):
    n = len(by_task)
    variance = 0.0
    for rewards in by_task.values():
        k = len(rewards)
        if k < 2:
            continue
        p = sum(1 for r in rewards if r and r > 0) / k
        variance += p * (1.0 - p) / (k - 1)
    return round(CI95_Z * 100.0 * (variance / (n * n)) ** 0.5, 2) if n else None


def _sum(rows, key):
    return sum(row.get(key) or 0 for row in rows)


def triples(rows):
    def sig(name):
        return sum((row["signals"].get(name) or 0) for row in rows)
    max_rung = max((row["signals"].get("intercept_max_rung") or 0 for row in rows), default=0)
    answer = [row["signals"].get("answer_shape") or 0 for row in rows]
    ttft = [row["ttftP50Ms"] for row in rows if row.get("ttftP50Ms") is not None]
    return (
        f"{sig('stopped_with_open_todos')} / {sig('intercept_count')}({max_rung}) / {sig('asked_twice')}",
        f"{sig('gate_without_change')} / {sig('done_without_check')} / {sig('count_claim')} / {sig('regression_seen_red')}",
        f"{round(statistics.mean(answer)) if answer else 0} / {sig('closing_offer')} / "
        f"{max((row['signals'].get('cache_miss_streak') or 0 for row in rows), default=0)} / "
        f"{(statistics.median(ttft) / 1000):.1f}s" if ttft else
        f"{round(statistics.mean(answer)) if answer else 0} / {sig('closing_offer')} / "
        f"{max((row['signals'].get('cache_miss_streak') or 0 for row in rows), default=0)} / -",
    )


def ledger_row(rows, suite, model, fingerprint, note):
    scored = [row for row in rows if row["reward"] is not None]
    by_task = {}
    for row in scored:
        by_task.setdefault(row["task"], []).append(row["reward"])
    passed = sum(1 for row in scored if row["reward"] and row["reward"] > 0)
    attempts = max((len(v) for v in by_task.values()), default=1)
    pk = pass_at_k(by_task)
    width = ci95(by_task) if attempts >= 2 else None
    pass_cell = f"k={attempts}" + (" " + " ".join(f"{k}={v}" for k, v in pk.items()) if pk else "") + (f" ±{width}" if width is not None else "")
    unknown = _sum(rows, "unknownTurns")
    cost = _sum(rows, "costUsd")
    wall = _sum(rows, "wallSec")
    persistence, rigor, experience = triples(rows)
    tallied = [row for row in rows if row.get("testsTotal") is not None]
    scores = [row["partialScore"] for row in rows if row.get("partialScore") is not None]
    partials = " ".join(([f"{_sum(tallied, 'testsPassed')}/{_sum(tallied, 'testsTotal')}"] if tallied else [])
                        + ([f"{statistics.mean(scores):.2f}"] if scores else [])) or "-"
    return " | ".join([
        "| NNNN", datetime.now().date().isoformat(), suite, model, fingerprint,
        f"{passed}/{len(scored)}" if scored else "—", pass_cell,
        f"{_sum(rows, 'input')}/{_sum(rows, 'cacheRead')}/{_sum(rows, 'output')}",
        "?" if unknown else (f"{cost:.4f}" if cost else "-"),
        f"{wall:.1f}s" if wall else "-", str(_sum(rows, "turns")),
        str(max((row["peakContextTokens"] or 0 for row in rows), default=0)),
        str(_sum(rows, "compactions")), note, "", persistence, rigor, experience,
        f"{_sum(rows, 'timedOut')}/{_sum(rows, 'verifierUnmeasured')}/{_sum(rows, 'errored')}", partials + " |",
    ])


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("root", type=Path)
    parser.add_argument("--suite", default="")
    parser.add_argument("--model", default="")
    parser.add_argument("--fingerprint", default="")
    parser.add_argument("--note", default="evals/axes.py")
    parser.add_argument("--json", type=Path, help="write the per-trial rows here as well")
    args = parser.parse_args(argv)
    root = args.root.resolve()
    sessions = sorted(path for path in root.rglob("*.jsonl") if is_session(path))
    if not sessions:
        print(f"no session files under {root}", file=sys.stderr)
        return 2
    rows = [score(path, root) for path in sessions]
    lines = [json.dumps(row, sort_keys=True) for row in rows]
    if args.json:
        args.json.write_text("\n".join(lines) + "\n")
    for line in lines:
        print(line)
    print(ledger_row(rows, args.suite or root.name, args.model, args.fingerprint, args.note))
    unmeasurable = [row["session"] for row in rows if not row["measurable"]]
    if unmeasurable:
        print(f"unmeasurable: {', '.join(unmeasurable)}", file=sys.stderr)
        return 2
    return 0


if __name__ == "__main__":
    sys.exit(main())
