#!/usr/bin/env python3
"""Tool-surface runner: one `yi ask --json` rollout per scenario, scored by the
refusals its sessions recorded rather than by a reward.

    python3 evals/surface.py --dry --binary target/debug/yi --model faux/faux-1
    python3 evals/surface.py --binary target/debug/yi \\
        --model openrouter/z-ai/glm-5.3-flash --cap-usd 3 --out runs/surface

The corpus behind it: 105 real sessions on 0.264.0 and 0.282.0 put the plan tool
at 39 percent refused, ipython at 17, bash at 15, edit at 12 and todo at 4, and
reading them one at a time produced #468 to #476. That reading is the loop this runs
(#477): exercise a tool road with a real agent, count what was refused, judge which
refusals were the caller's fault and which were the tool's, fix, run it again.
The rate is the number a release is judged by.

Scenarios are data (`fixtures/surface/scenarios.json`): a road, a prompt, the
seed files it needs and what a clean run looks like. Adding a road is a JSON
entry, never an edit here.

Counting is deterministic and belongs to `skills/yi/session-mining/extract.py`,
whose `issues.jsonl` already groups refusals by tool with the text and whether
the session recovered. Judging a refusal correct or not is a reading act and
stays a human's: this prints the evidence and leaves the verdict blank. The
runner never guesses what a caller meant.

`--dry` is faux only and refuses any other provider, because a gate spends no
API budget; it proves the plumbing and the scenario schema, and rides
`just postmerge-evals` beside `run.py --dry` because it needs a built binary.
`selftest.py::check_surface` covers the schema and the census with no binary at
all. A real-model run is user-run, capped and ledgered (README, plan law 3).
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "skills" / "yi" / "session-mining"))

import extract  # noqa: E402
import run as runner  # noqa: E402
import yi_usage  # noqa: E402

SCENARIOS = ROOT / "fixtures" / "surface" / "scenarios.json"
SCENARIO_KEYS = {"id", "road", "prompt", "timeoutSec", "seed", "clean", "levers"}
REQUIRED_KEYS = {"id", "road", "prompt", "timeoutSec", "seed", "clean"}
CLEAN_KEYS = {"files", "noRefusalsFrom"}
QUOTE_CHARS = 220


def load_scenarios(path=SCENARIOS):
    """Every scenario, schema-checked. A malformed entry is refused by name: a
    scenario nobody can run is worse than one nobody added."""
    doc = json.loads(Path(path).read_text())
    scenarios = doc.get("scenarios")
    if not isinstance(scenarios, list) or not scenarios:
        raise ValueError(f"{path}: scenarios is not a non-empty list")
    seen = set()
    for scenario in scenarios:
        if not isinstance(scenario, dict):
            raise ValueError(f"{path}: a scenario is not an object")
        name = scenario.get("id")
        missing = REQUIRED_KEYS - set(scenario)
        unknown = set(scenario) - SCENARIO_KEYS
        if missing:
            raise ValueError(f"{path}: scenario {name!r} is missing {sorted(missing)}")
        if unknown:
            raise ValueError(f"{path}: scenario {name!r} has unknown {sorted(unknown)}")
        if name in seen:
            raise ValueError(f"{path}: scenario id {name!r} is used twice")
        seen.add(name)
        if not isinstance(scenario["seed"], dict):
            raise ValueError(f"{path}: scenario {name!r} seed is not an object")
        if not isinstance(scenario["timeoutSec"], int) or scenario["timeoutSec"] <= 0:
            raise ValueError(f"{path}: scenario {name!r} timeoutSec is not a positive integer")
        clean = scenario["clean"]
        if not isinstance(clean, dict) or set(clean) - CLEAN_KEYS:
            raise ValueError(f"{path}: scenario {name!r} clean is not {sorted(CLEAN_KEYS)}")
        for key in CLEAN_KEYS:
            if not isinstance(clean.get(key, []), list):
                raise ValueError(f"{path}: scenario {name!r} clean.{key} is not a list")
    return scenarios


def run_scenario(scenario, binary, model, out, cap_usd=None, spent=0.0):
    """One rollout in its own workspace and its own HOME, never the caller's
    `~/.yi`. A timeout is a result and never a retry (README, budget discipline)."""
    started = time.monotonic()
    name = scenario["id"]
    keep = Path(out) / name
    keep.mkdir(parents=True, exist_ok=True)
    home = tempfile.mkdtemp(prefix=f"yi-surface-home-{name}-")
    work = tempfile.mkdtemp(prefix=f"yi-surface-{name}-")
    try:
        config = Path(home) / ".yi" / "config.json"
        config.parent.mkdir(parents=True, exist_ok=True)
        # yi_usage.eval_config is the one writer of a trial HOME's config, as it is
        # for run.py and the harbor adapter.
        config.write_text(json.dumps(yi_usage.eval_config({})))
        workspace = Path(work) / "repo"
        workspace.mkdir()
        for relative, body in scenario["seed"].items():
            target = workspace / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_text(body)
        # A worktree delegation spawns only in a repository, so the workspace is one.
        for git in (["init", "-q", "-b", "main"], ["add", "-A"],
                    ["-c", "user.name=s", "-c", "user.email=s@example.invalid",
                     "commit", "-q", "--allow-empty", "-m", "seed"]):
            subprocess.run(["git", *git], cwd=workspace, check=True)
        sessions = keep / "sessions"
        events = keep / "events.jsonl"
        environment = {"HOME": home}
        command = [
            binary, "ask", "--model", model, "--json", "--here", "--yolo",
            "--cwd", str(workspace), "--session-dir", str(sessions),
            "--deadline", str(scenario["timeoutSec"]),
        ]
        if scenario.get("levers"):
            levers = keep / "levers.json"
            levers.write_text(json.dumps(scenario["levers"]))
            # The binary reads YI_LEVERS only under --eval, and only a harness
            # passes it (D220).
            command.append("--eval")
            environment[yi_usage.LEVERS_ENV] = str(levers)
        command.append(scenario["prompt"])
        timed_out, exit_code = False, None
        with events.open("w") as sink, (keep / "stderr.log").open("w") as stderr:
            try:
                exit_code = subprocess.run(
                    command, stdout=sink, stderr=stderr, stdin=subprocess.DEVNULL,
                    env={**os.environ, **environment},
                    timeout=scenario["timeoutSec"] + 120,
                ).returncode
            except subprocess.TimeoutExpired:
                timed_out = True
        shutil.copytree(workspace, keep / "repo", dirs_exist_ok=True)
        row = {
            "scenario": name,
            "road": scenario["road"],
            "exit": exit_code,
            "timedOut": timed_out,
            "wallSec": round(time.monotonic() - started, 1),
            "missingFiles": [
                path for path in scenario["clean"].get("files", [])
                if not (workspace / path).exists()
            ],
        }
        row.update(yi_usage.parse_events(events))
        return row
    finally:
        shutil.rmtree(home, ignore_errors=True)
        shutil.rmtree(work, ignore_errors=True)


def collect_sessions(out):
    """Every session file the rollouts wrote, named by the scenario that wrote it,
    in one directory the extractor can sweep."""
    corpus = Path(out) / "sessions"
    if corpus.exists():
        shutil.rmtree(corpus)
    corpus.mkdir(parents=True)
    for path in sorted(Path(out).glob("*/sessions/**/*.jsonl")):
        if path.name.endswith(".telemetry.jsonl"):
            continue
        scenario = path.relative_to(out).parts[0]
        shutil.copy(path, corpus / f"{scenario}--{path.name}")
    return corpus


def mine(corpus, out):
    """The extractor owns the schema; this reads its store rather than the sessions."""
    store = Path(out) / "mining"
    result = subprocess.run(
        [sys.executable, str(ROOT.parent / "skills/yi/session-mining/extract.py"),
         "--sessions", str(corpus), "--out", str(store)],
        capture_output=True, text=True, check=False,
    )
    if result.returncode != 0:
        raise RuntimeError(f"the extractor refused the corpus: {result.stderr.strip()}")
    return store


def _read(store, name):
    path = Path(store) / name
    if not path.is_file():
        return []
    return [json.loads(line) for line in path.read_text().splitlines() if line.strip()]


# The tool's own `errorKind` names the class; a command's exit and a kernel call into rlm/yi
# (`api_misuse`, which holds verdicts and misreads alike) are the extractor's blames.
# Only misread and stale count toward the zero target; `untagged` is a tool not yet saying.
CLASS_OF_KIND = {
    "invalid_args": "misread", "noop_loop": "misread",
    "stale": "stale", "stale_tag": "stale", "not_found": "stale",
    "verdict": "verdict", "denied": "safety", "aborted": "tool", "tool_error": "tool",
}
CLASSES = ("misread", "stale", "verdict", "exit", "api_misuse", "safety", "tool", "untagged")


def class_of(issue):
    blame = issue.get("errorClass")
    if blame == "command":
        return "exit"
    if blame == "api_misuse":
        return "api_misuse"
    return CLASS_OF_KIND.get(issue.get("errorKind") or "", "untagged")


def census(store):
    """Per-tool calls and refusals, both from the extractor's own store: `mu.jsonl`
    carries `toolCalls.byTool` and `issues.jsonl` groups every refusal by tool."""
    calls, refusals, classes = {}, {}, dict.fromkeys(CLASSES, 0)
    for row in _read(store, "mu.jsonl"):
        for tool, count in (row.get("toolCalls") or {}).get("byTool", {}).items():
            calls[tool] = calls.get(tool, 0) + count
    for issue in _read(store, "issues.jsonl"):
        tool = issue.get("tool") or "?"
        refusals[tool] = refusals.get(tool, 0) + issue.get("count", 0)
        classes[class_of(issue)] += issue.get("count", 0)
    return {
        "calls": dict(sorted(calls.items())),
        "refusals": dict(sorted(refusals.items())),
        "classes": classes,
        "rate": {
            tool: round(refusals.get(tool, 0) / count, 4)
            for tool, count in sorted(calls.items()) if count
        },
    }


def ledger_cell(counts):
    """The eval ledger's tool-failures cell: the two counted classes, then the rest, of all calls."""
    classes = counts["classes"]
    rest = ", ".join(f"{name} {classes[name]}" for name in CLASSES[2:])
    return (f"tool failures: misread {classes['misread']}, stale {classes['stale']}"
            f"; {rest} of {sum(counts['calls'].values())} calls")


def frictions(corpus, store, sessions, limit=2):
    """The model's own words beside a refusal it met: the thinking of the turn the
    refused call was made in, redacted by the extractor's own vocabulary."""
    files = {row.get("sessionId"): row.get("file") for row in _read(store, "mu.jsonl")}
    found = []
    for session in sessions:
        path = Path(corpus) / (files.get(session) or "")
        if not path.is_file():
            continue
        _header, entries, _corrupt = extract.read_session(path)
        pending = ""
        for entry in entries:
            message = entry.get("message") or {}
            blocks = message.get("content")
            if message.get("role") == "assistant" and isinstance(blocks, list):
                pending = " ".join(
                    " ".join(
                        (block.get("thinking") or block.get("content") or "").split()
                    )
                    for block in blocks
                    if isinstance(block, dict) and block.get("type") == "thinking"
                ).strip()
            elif message.get("role") == "toolResult" and message.get("isError") and pending:
                found.append(extract.redact(pending)[:QUOTE_CHARS])
                pending = ""
                if len(found) >= limit:
                    return found
    return found


def report(rows, store, corpus):
    """What a human reads: refusals grouped by tool, with the evidence and a blank
    verdict. Which ones were the caller's fault is the reading pass, not this."""
    lines = ["", ledger_cell(census(store)), "", "refusals by tool (judge each: caller's mistake, or the tool's)"]
    issues = sorted(_read(store, "issues.jsonl"), key=lambda issue: -issue.get("count", 0))
    if not issues:
        lines.append("  none")
    for issue in issues:
        lines.append(
            f"  {issue.get('tool'):8s} x{issue.get('count', 0):<3d} {issue.get('state', '?'):8s}"
            f" correct? ____   {issue.get('example', '')[:120]}"
        )
        resolution = issue.get("resolution") or {}
        lines.append(
            f"           recovered {resolution.get('pivot', 0)}, unresolved"
            f" {resolution.get('unresolved', 0)}; sessions {len(issue.get('sessions') or [])}"
        )
        for quote in frictions(corpus, store, issue.get("sessions") or []):
            lines.append(f'           model: "{quote}"')
    lines.append("")
    for row in rows:
        marks = []
        if row.get("timedOut"):
            marks.append("timed out")
        if row.get("missingFiles"):
            marks.append(f"no {', '.join(row['missingFiles'])}")
        lines.append(f"  {row['scenario']:22s} {row['road']}" + (f"  [{'; '.join(marks)}]" if marks else ""))
    return "\n".join(lines)


def clean(row):
    return row.get("exit") == 0 and not row.get("timedOut") and not row.get("missingFiles")


def compare(base, candidate):
    """N6 (docs/plans/2026-09-26-self-improvement-evals.md 6.6): the reason a tool-text candidate
    is refused against its base, or None. Each side is a list of `surface.json` documents from
    runs made alternately with the base's. Refused when any tool's refusal rate over the side's
    summed calls rises, or when a scenario clean on every base run is unclean on a candidate one."""
    def rates(docs):
        calls, refusals = {}, {}
        for doc in docs:
            for tool, count in doc["toolSurface"]["calls"].items():
                calls[tool] = calls.get(tool, 0) + count
            for tool, count in doc["toolSurface"]["refusals"].items():
                refusals[tool] = refusals.get(tool, 0) + count
        return {tool: refusals.get(tool, 0) / calls[tool] for tool in calls if calls[tool]}

    was, now = rates(base), rates(candidate)
    for tool in sorted(now):
        if now[tool] > was.get(tool, 0.0):
            return f"refusal_rate_rose:{tool}"
    base_rows = [row for doc in base for row in doc.get("rows", [])]
    always = {row["scenario"] for row in base_rows} - {row["scenario"] for row in base_rows if not clean(row)}
    for row in (row for doc in candidate for row in doc.get("rows", [])):
        if row["scenario"] in always and not clean(row):
            return f"scenario_unclean:{row['scenario']}"
    return None


def main(argv=None):
    argv = sys.argv[1:] if argv is None else argv
    if argv[:1] == ["compare"]:
        pair = argparse.ArgumentParser(description=compare.__doc__.splitlines()[0])
        pair.add_argument("--base", action="append", required=True, help="a base run's surface.json")
        pair.add_argument("--candidate", action="append", required=True, help="a candidate run's surface.json")
        sides = pair.parse_args(argv[1:])
        read = lambda paths: [json.loads(Path(path).read_text()) for path in paths]
        reason = compare(read(sides.base), read(sides.candidate))
        print(json.dumps({"verdict": "refused" if reason else "pass", "reason": reason}))
        return 1 if reason else 0
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", default=str(ROOT.parent / "target/debug/yi"))
    parser.add_argument("--model", required=True)
    parser.add_argument("--out", default=None, help="where rollouts are kept; a temp dir otherwise")
    parser.add_argument("--only", action="append", help="scenario id, or a prefix ending in *; repeatable")
    parser.add_argument("--cap-usd", type=float, default=None, help="stop before a rollout that would pass it")
    parser.add_argument("--dry", action="store_true", help="faux only: plumbing and schema, no key, no spend")
    parser.add_argument("--selfcheck", action="store_true", help="schema and census, no binary")
    args = parser.parse_args(argv)

    try:
        scenarios = load_scenarios()
    except ValueError as error:
        print(f"refused: {error}", file=sys.stderr)
        return 2
    if args.selfcheck:
        print(f"ok   surface ({len(scenarios)} scenarios)")
        return 0

    # Preconditions by name, never a key value (evals/drivers/README.md).
    if args.dry and not args.model.startswith("faux/"):
        print(f"refused: --dry runs faux only, not {args.model}; a gate spends no API budget", file=sys.stderr)
        return 2
    if not args.dry:
        if not os.environ.get("OPENROUTER_API_KEY"):
            print("refused: OPENROUTER_API_KEY is unset", file=sys.stderr)
            return 2
        if args.cap_usd is None:
            print("refused: --cap-usd is required for a real-model run (plan law 3)", file=sys.stderr)
            return 2
    if not Path(args.binary).is_file():
        print(f"refused: no binary at {args.binary} (cargo build -p yi-cli)", file=sys.stderr)
        return 2

    picked = lambda s: s["id"] in args.only or any(
        s["id"].startswith(prefix[:-1]) for prefix in args.only if prefix.endswith("*"))
    chosen = [s for s in scenarios if not args.only or picked(s)]
    if not chosen:
        print(f"refused: no scenario matches {args.only}", file=sys.stderr)
        return 2

    with tempfile.TemporaryDirectory(prefix="yi-surface-") as scratch:
        out = Path(args.out) if args.out else Path(scratch)
        out.mkdir(parents=True, exist_ok=True)
        rows, spent, stopped = [], 0.0, None
        for scenario in chosen:
            if args.cap_usd is not None and spent >= args.cap_usd:
                stopped = f"cap ${args.cap_usd} reached at ${spent:.4f} before {scenario['id']}"
                break
            row = run_scenario(scenario, args.binary, args.model, out)
            cost = row.get("costUsd")
            spent += cost if isinstance(cost, (int, float)) else 0.0
            row["spentUsd"] = round(spent, 4)
            rows.append(row)
            print(json.dumps(row), flush=True)
        corpus = collect_sessions(out)
        store = mine(corpus, out)
        counts = census(store)
        machine = {"scenarios": len(rows), "model": args.model, "toolSurface": counts,
                   "spentUsd": round(spent, 4), "stopped": stopped,
                   "rows": [{key: row.get(key) for key in ("scenario", "exit", "timedOut", "missingFiles")}
                            for row in rows]}
        (out / "surface.json").write_text(json.dumps(machine, indent=1) + "\n")
        print(json.dumps(machine), flush=True)
        print(report(rows, store, corpus))
        if stopped:
            print(f"stopped: {stopped}", file=sys.stderr)
        if args.dry:
            # Faux calls no tool, so the dry tier pins the plumbing: every rollout
            # ran, every session was swept, and the census exists.
            missing = [row["scenario"] for row in rows if row.get("exit") != 0]
            if missing:
                print(f"FAIL surface_dry: rollouts did not exit 0: {missing}")
                return 1
            if not counts["calls"] and not counts["refusals"]:
                print("ok   surface_dry (faux called no tool, as it never does)")
            return 0
        fingerprint = yi_usage.config_fingerprint(
            runner._capture([args.binary, "--version"]), args.model, "surface", "surface"
        )
        print(runner.ledger_row(rows, args.model, fingerprint, "surface", "evals/surface.py"))
    return 0


if __name__ == "__main__":
    sys.exit(main())
