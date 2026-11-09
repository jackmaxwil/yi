#!/usr/bin/env python3
"""The weekly digest: verdicts, prefix growth and where real sessions spend tokens.

    python3 evals/report.py [--verdicts DIR/verdicts.json ...] [--sessions DIR ...] [--html FILE] [--since 8]

The terminal form is a markdown digest of rows at most 80 columns wide, which the weekly job posts
as one comment. `--html` writes the same facts as one self-contained file with inline SVG charts,
no script and no address, so it opens offline from the job's artifact. Inputs are files Yi already
writes: weekly.py's verdicts, the git history of the request budget (the static prefix re-read on
every request) and the session-mining census over any session directories given.
"""
import argparse, html, json, pathlib, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT / "skills/yi/session-mining"))
BUDGET = "scripts/guardrails/baselines/request_budget.json"
WIDTH = 80


def prefix_history(since):
    """(short sha, date, total bytes) for the last `since` commits that moved the request budget."""
    log = subprocess.run(["git", "-C", str(ROOT), "log", f"-{since}", "--format=%h %cs", "--", BUDGET],
                         capture_output=True, text=True, check=False).stdout.split("\n")
    points = []
    for line in filter(None, log):
        sha, date = line.split()
        shown = subprocess.run(["git", "-C", str(ROOT), "show", f"{sha}:{BUDGET}"],
                               capture_output=True, text=True, check=False).stdout
        try:
            points.append((sha, date, int(json.loads(shown)["total"])))
        except (ValueError, KeyError, TypeError):
            continue
    return list(reversed(points))


def spark(values):
    bars = "▁▂▃▄▅▆▇█"
    if not values:
        return ""
    low, high = min(values), max(values)
    return "".join(bars[0 if high == low else round((v - low) / (high - low) * (len(bars) - 1))] for v in values)


def verdict_rows(paths):
    rows = []
    for path in paths:
        summary = json.loads(pathlib.Path(path).read_text())
        for leg in summary.get("legs", []):
            verdict = leg.get("verdict") or {}
            graded = (verdict.get("intervals") or {}).get("graded") or {}
            rows.append({"run": summary.get("runId"), "leg": leg["leg"], "verdict": verdict.get("verdict") or leg.get("stopped") or "-",
                         "by": verdict.get("by") or "-", "unit": verdict.get("unit") or "-",
                         "graded": graded.get("median"), "low": graded.get("low"), "high": graded.get("high"),
                         "spent": leg.get("spentUsd", 0.0), "tasks": leg.get("tasks", 0)})
    return rows


def sinks(directories, top=5):
    """The costliest sessions by prompt tokens, as counts: no path or prompt text is printed."""
    if not directories:
        return []
    import extract
    mu = extract.sweep([pathlib.Path(d) for d in directories], pathlib.Path(tempfile.mkdtemp()) / "mining")["mu"]
    ranked = sorted(mu, key=lambda r: r["tokens"]["input"] + r["tokens"]["cacheRead"], reverse=True)
    return [{"session": r["sessionId"][:8], "turns": r["turns"],
             "prompt": r["tokens"]["input"] + r["tokens"]["cacheRead"], "compactions": r["compactions"],
             "stale": r["harness"]["stale_run"], "recheck": r["harness"]["resolved_recheck"]} for r in ranked[:top]]


def number(value, fmt="{:+.3f}"):
    return "-" if value is None else fmt.format(value)


def digest(verdicts, prefix, top):
    out = ["## Weekly evals", "", "| run | leg | verdict | by | graded Δ [low, high] | $ |", "|---|---|---|---|---|---|"]
    for row in verdicts:
        span = f"{number(row['graded'])} [{number(row['low'])}, {number(row['high'])}]"
        out.append(f"| {row['run']} | {row['leg']} | {row['verdict'][:24]} | {row['by']} | {span} | {row['spent']:.2f} |")
    if not verdicts:
        out.append("| - | - | no verdicts | - | - | - |")
    out += ["", "## Static prefix (request budget, bytes)", ""]
    if prefix:
        first, last = prefix[0][2], prefix[-1][2]
        out.append(f"{spark([p[2] for p in prefix])} {first} -> {last} ({last - first:+d}) over {len(prefix)} moves")
        out.append(f"latest {prefix[-1][0]} {prefix[-1][1]}")
    else:
        out.append("no history")
    out += ["", "## Token sinks (sessions)", ""]
    if top:
        out += ["| session | turns | prompt tokens | compactions | stale runs | re-checks |", "|---|---|---|---|---|---|"]
        out += [f"| {s['session']} | {s['turns']} | {s['prompt']} | {s['compactions']} | {s['stale']} | {s['recheck']} |" for s in top]
    else:
        out.append("no session directories given")
    return "\n".join(line[:WIDTH] for line in out)


def svg_line(points, title):
    if len(points) < 2:
        return f"<p>{html.escape(title)}: fewer than two points</p>"
    w, h, pad = 560, 140, 24
    low, high = min(points), max(points)
    span = (high - low) or 1
    xs = [pad + i * (w - 2 * pad) / (len(points) - 1) for i in range(len(points))]
    ys = [h - pad - (v - low) * (h - 2 * pad) / span for v in points]
    path = " ".join(f"{x:.1f},{y:.1f}" for x, y in zip(xs, ys))
    return (f'<figure><figcaption>{html.escape(title)}</figcaption>'
            f'<svg viewBox="0 0 {w} {h}" width="{w}" height="{h}" role="img">'
            f'<polyline fill="none" stroke="currentColor" stroke-width="2" points="{path}"/>'
            f'<text x="{pad}" y="14" font-size="11">{high}</text><text x="{pad}" y="{h - 4}" font-size="11">{low}</text>'
            f'</svg></figure>')


def page(verdicts, prefix, top, text):
    charts = [svg_line([p[2] for p in prefix], "Static prefix bytes per budget move")]
    medians = [row["graded"] for row in verdicts if row["graded"] is not None]
    charts.append(svg_line(medians, "Weekly graded difference, head minus base"))
    return ("<!doctype html><meta charset=utf-8><title>Yi weekly evals</title>"
            "<style>body{font:14px system-ui;max-width:720px;margin:2em auto;padding:0 16px}"
            "pre{white-space:pre-wrap}</style>" + "".join(charts) + f"<pre>{html.escape(text)}</pre>")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--verdicts", nargs="*", default=[])
    parser.add_argument("--sessions", nargs="*", default=[])
    parser.add_argument("--html", type=pathlib.Path)
    parser.add_argument("--since", type=int, default=8, help="budget moves to chart")
    args = parser.parse_args(argv)
    verdicts, prefix, top = verdict_rows(args.verdicts), prefix_history(args.since), sinks(args.sessions)
    text = digest(verdicts, prefix, top)
    print(text)
    if args.html:
        args.html.write_text(page(verdicts, prefix, top, text))
    return 0


if __name__ == "__main__":
    sys.exit(main())
