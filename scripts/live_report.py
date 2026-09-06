#!/usr/bin/env python3
"""The live lane's PR comment, from `run.json`: statuses, error classes, the numbers a run is
judged by. The first line is the upsert marker forgejo_pr_comment.py keys on, so it never varies.
"""
import json, pathlib, sys

MARKER = "Live lane — the real binary against a real model, judged by verifiers and `yi doctor`."


def render(record, verdict=None, baseline=None):
    counts = record.get("counts", {})
    telemetry = record.get("telemetry", {})
    lines = [MARKER, ""]
    if record.get("skipped"):
        lines.append(f"**inconclusive** — {record['skipped']}")
        return "\n".join(lines) + "\n"
    if verdict is not None:
        lines.append("**RED** — " + "; ".join(verdict.get("findings", [])) if verdict.get("red")
                     else "**green** — every band and ratchet holds"
                     + (f" against {baseline['runs']} run(s) of history" if baseline and baseline.get("runs") else " (no history yet)"))
        lines.append("")
    lines.append(
        f"`{record.get('model', '?')}` · {counts.get('pass', 0)} pass · {counts.get('fail', 0)} fail · "
        f"{counts.get('inconclusive', 0)} inconclusive · spent ${record.get('spentUsd', 0):.4f} of "
        f"${record.get('capUsd', 0):.2f}" + (" · **budget hit**" if record.get("budgetHit") else "")
    )
    lines += ["", "| scenario | status | detail | cost |", "|---|---|---|---|"]
    for row in record.get("rows", []):
        cost = row.get("costUsd")
        lines.append(
            f"| {row.get('task', '?')} | {row.get('status', '?')} | {row.get('detail', '') or ''} | "
            f"{'' if cost is None else f'${cost:.4f}'} |"
        )
    if telemetry:
        lines += [
            "",
            f"requests {telemetry.get('requests', 0)} · ttft p50 {telemetry.get('ttftP50Ms', 0)} ms · "
            f"p95 {telemetry.get('ttftP95Ms', 0)} ms · total p50 {telemetry.get('totalP50Ms', 0)} ms · "
            f"cache hit {100 * float(telemetry.get('hitRate', 0) or 0):.1f}% · "
            f"cost ${float(telemetry.get('costUsd', 0) or 0):.4f}",
        ]
        classes = telemetry.get("classes") or {}
        if classes:
            lines.append("classes: " + ", ".join(f"`{name}` ×{count}" for name, count in sorted(classes.items())))
    return "\n".join(lines) + "\n"


def selfcheck():
    text = render({"model": "m", "counts": {"pass": 1, "fail": 0, "inconclusive": 1}, "spentUsd": 0.01, "capUsd": 1,
                   "rows": [{"task": "a", "status": "pass", "detail": "", "costUsd": 0.01},
                            {"task": "b", "status": "inconclusive", "detail": "budget"}],
                   "telemetry": {"requests": 1, "ttftP50Ms": 800, "ttftP95Ms": 800, "totalP50Ms": 4000,
                                 "hitRate": 0.5, "costUsd": 0.01, "classes": {"Invariant::lanes": 1}}})
    assert text.startswith(MARKER + "\n"), "the marker is the first line"
    assert "| a | pass |  | $0.0100 |" in text and "| b | inconclusive | budget |  |" in text, text
    assert "cache hit 50.0%" in text and "`Invariant::lanes` ×1" in text, text
    red = render({"model": "m", "counts": {}, "rows": []}, {"red": True, "findings": ["band: ttft p50 3500 ms > 3000 ms"]}, {"runs": 3})
    assert "**RED** — band: ttft" in red, red
    green = render({"model": "m", "counts": {}, "rows": []}, {"red": False, "findings": []}, {})
    assert "**green**" in green and "no history yet" in green, green
    skipped = render({"skipped": "no OPENROUTER_API_KEY secret"})
    assert skipped.startswith(MARKER) and "**inconclusive**" in skipped
    print("ok   live_report selfcheck")


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        selfcheck()
        sys.exit(0)
    if len(sys.argv) not in (2, 4):
        print("usage: live_report.py <run.json> [<verdict.json> <baseline.json>]", file=sys.stderr)
        sys.exit(2)
    record = json.loads(pathlib.Path(sys.argv[1]).read_text())
    verdict = baseline = None
    if len(sys.argv) == 4:
        verdict = json.loads(pathlib.Path(sys.argv[2]).read_text())
        baseline = json.loads(pathlib.Path(sys.argv[3]).read_text()) if pathlib.Path(sys.argv[3]).is_file() else {}
    sys.stdout.write(render(record, verdict, baseline))
