"""EXP-S5-E2: do children that keep a kernel beat bash workers on the queue tasks?

Three Terminal-Bench v4 shape-G tasks x 3 repeats x 3 arms: solo; a parent with gathered `worker`
children; a parent with gathered `root` children, which keep their own kernel. An arm is `EVAL_ARM`
(the row label) and `EVAL_ARM_FILE` (the text the harbor adapter appends to the instruction); the
sweep runs one arm per call, `-k 3` over the three tasks.

    python3 evals/kill/s5-kc-ab.py precheck [out dir]   oracle and nop once per task, zero spend
    python3 evals/kill/s5-kc-ab.py table [run id]...    one JSON row per trial

The pre-registered score is the ctrf tally, passed/tests. Only medical-claims-processing writes a
ctrf (one test), and heat-pump-warranty and legacy-utility-triage write none, so `tally` reads the
verifier's own count of what the agent got right: trace_results.json's cases, else the pytest
line `Scoring: X/Y lines correct`, else the ctrf. The "fewer than 10 tests" guard counts that
unit: a task whose verifier cannot score in at least 10 steps leaves the verdict UNRESOLVED.
"""
import json, math, os, platform, re, subprocess, sys, tempfile
from fractions import Fraction
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))

import axes  # noqa: E402

THRESHOLD = ("BUILD iff on K3.1 mean(K - W) >= 0.20 with one-sided 95% LB > 0, K passes >= W passes and "
             "K >= solo, then on flash mean >= 0.10 and passes >= W; KILL otherwise; UNRESOLVED if SD(d) > "
             "0.35 or ctrf holds < 10 tests; MDE 0.23 at SD 0.25")
FAMILIES = ("kimi", "glm-flash")
# Kimi is the subscription's $0; its figure is the OpenRouter K3.1 price, about 40x flash's $5.38.
PREDICTED_USD = {"kimi": 215.2, "glm-flash": 5.38}
STEP = 9
TASKS = ("heat-pump-warranty", "legacy-utility-triage", "medical-claims-processing")
REPEATS = 3
ARMS = ("solo", "workers", "kernel")
ROUTING = {"ignore": ["inference-net"]}
MIN_UNITS = 10
# Student t, one-sided 95%, 8 degrees of freedom: 3 tasks x 3 repeats is nine pairs.
T95_DF8 = 1.8595
SD_MAX = Fraction(35, 100)
LINES = re.compile(r"Scoring:\s*(\d+)/(\d+) lines correct")

CHILDREN = """\
This task is a queue of items worked under one rulebook. Work it as a parent with children.

1. Read the rulebook and the item list yourself first. Find what ties two items together: a shared site, account, asset or invoice, or an earlier decision that a later one cites.
2. Put the items in groups so that no item depends on anything outside its group. Use as few groups as that allows, and at most eight.
3. Start one child per group, all together in one cell, with the rulebook as the partition:

       import asyncio
       handles = await asyncio.gather(*(rlm.run(brief(group), role="{role}"{options}, partition=[RULEBOOK]) for group in groups))
       await rlm.wait(300)

   RULEBOOK is `local://` plus the path of the file that holds the rules. Each brief names the group's item ids and the sources to read, and asks for one JSON draft per item: the decision, every field the rules ask for, and the evidence ids to cite.
4. A child drafts and reports. It never submits, saves, clicks or changes any state that cannot be undone. Only you do, and only after you have read its draft against the rules.
5. Submit the decisions yourself, one at a time, in dependency order: a later decision may cite an id that an earlier submission creates. Only one desktop or portal session exists, so you drive it alone.
"""
ARM_TEXT = {"solo": None,
            "workers": CHILDREN.format(role="worker", options=', tools=["read", "grep", "bash"], turns=30'),
            "kernel": CHILDREN.format(role="root", options="")}


def runs():
    """One sweep per arm. The solo arm sets no arm file: its instruction is the task's own."""
    scratch = Path(tempfile.mkdtemp(prefix="s5-kc-ab-"))
    (scratch / "defaults.json").write_text("{}")
    out = []
    for arm in ARMS:
        env = [f"EVAL_ARM={arm}", f"EVAL_ROUTING={json.dumps(ROUTING)}", f"TBV4_ATTEMPTS={REPEATS}"]
        if ARM_TEXT[arm]:
            (scratch / f"{arm}.md").write_text(ARM_TEXT[arm])
            env.append(f"EVAL_ARM_FILE={scratch / f'{arm}.md'}")
        else:
            env = ["-u", "EVAL_ARM_FILE", *env]
        out.append({"id": arm, "harbor": ["env", *env, "sh", "evals/drivers/tbv4_sweep.sh", "--runner",
                                         str(scratch / "defaults.json"), *TASKS]})
    return out


def runs_root():
    """Where tbv4_sweep.sh --runner keeps a run's trials, by its own rule."""
    return Path(os.environ.get("TBV4_RUNS_ROOT") or Path.home() / "Development" / "yi-runs")


def tally(trial):
    """(passed, units, source) from the verifier's own files in a trial dir, or None."""
    trace = axes.verifier_report(trial, "trace_results.json")
    if isinstance(trace.get("passed_cases"), int) and isinstance(trace.get("total_cases"), int):
        return trace["passed_cases"], trace["total_cases"], "trace_results.json"
    stdout = trial / "verifier" / "test-stdout.txt"
    lines = LINES.search(stdout.read_text(errors="replace")) if stdout.is_file() else None
    if lines:
        return int(lines[1]), int(lines[2]), "test-stdout.txt"
    ctrf = (axes.verifier_report(trial, "ctrf.json").get("results") or {}).get("summary") or {}
    if isinstance(ctrf.get("tests"), int):
        return ctrf.get("passed") or 0, ctrf["tests"], "ctrf.json"
    return None


def is_root(session):
    with session.open(errors="replace") as handle:
        return not json.loads(handle.readline()).get("parentSessionId")


def trial_row(trial, run):
    """One trial: its score and pass, the parent's peak context and the children's input tokens
    (a session records tokens, not request bytes)."""
    context, got = axes.harbor_context(trial), tally(trial)
    peak = child_input = children = 0
    for session in axes.scored_sessions(trial):
        row = axes.score(session, trial)
        if is_root(session):
            peak = max(peak, row["peakContextTokens"] or 0)
        else:
            children += 1
            child_input += (row["input"] or 0) + (row["cacheRead"] or 0)
    return {"run": run, "arm": run.rsplit("-", 1)[1], "task": (context["task"] or "").rsplit("/", 1)[-1],
            "trial": trial.name, "pass": context["reward"] >= 1, "passed": got[0] if got else None,
            "units": got[1] if got else 0, "source": got[2] if got else None,
            "score": Fraction(got[0], got[1]) if got and got[1] else None,
            "errored": context["errored"], "timedOut": context["timedOut"], "censored": context["censored"],
            "parentPeakTokens": peak, "children": children, "childInputTokens": child_input}


def trials_of(run):
    """The trials of a run id in a fixed order: job directory, then trial name. A job's own
    result.json has no result.json above it."""
    return [trial_row(path.parent, run) for path in sorted((runs_root() / run).rglob("result.json"))
            if (path.parent.parent / "result.json").is_file()]


def judge(arms):
    """The rule's numbers for one family from {arm: [trial rows]}. The first three scored trials
    of each arm and task are its repeats, so the pairs are (task, repeat) across arms. `open` lists
    what stops a verdict: a missing trial, a thin verifier or a spread past SD_MAX."""
    out = {"open": [], "passes": {}, "mean": {}}
    cells = {}
    for arm in ARMS:
        for task in TASKS:
            mine = [r for r in arms.get(arm, ()) if r["task"] == task and r["score"] is not None and not r["errored"]]
            cells[arm, task] = mine[:REPEATS]
            if len(mine) < REPEATS:
                out["open"].append(f"{arm} {task} has {len(mine)} of {REPEATS} scored trials")
    out["open"] += [f"{task} is scored in only {units} step{'s' * (units != 1)}" for task, units in
                    sorted({(r["task"], r["units"]) for rows in cells.values() for r in rows if r["units"] < MIN_UNITS})]
    if out["open"]:
        return out
    rows = {arm: [r for task in TASKS for r in cells[arm, task]] for arm in ARMS}
    out["mean"] = {arm: sum(r["score"] for r in rows[arm]) / len(rows[arm]) for arm in ARMS}
    out["passes"] = {arm: sum(r["pass"] for r in rows[arm]) for arm in ARMS}
    d = [k["score"] - w["score"] for k, w in zip(rows["kernel"], rows["workers"])]
    mean = sum(d) / len(d)
    var = sum((x - mean) ** 2 for x in d) / (len(d) - 1)
    out.update(d=mean, sd=math.sqrt(var), lb=float(mean) - T95_DF8 * math.sqrt(var / len(d)))
    if var > SD_MAX ** 2:
        out["open"].append(f"SD(d) {out['sd']:.2f} is above {float(SD_MAX)}")
    return out


def clears(family, got):
    kernel, workers = got["passes"]["kernel"], got["passes"]["workers"]
    if family == "kimi":
        return (got["d"] >= Fraction(1, 5) and got["lb"] > 0 and kernel >= workers
                and got["mean"]["kernel"] >= got["mean"]["solo"])
    return got["d"] >= Fraction(1, 10) and kernel >= workers


def describe(family, got):
    if got["open"]:
        return f"{family} open: " + ", ".join(got["open"])
    p, m = got["passes"], got["mean"]
    return (f"{family} d {float(got['d']):+.2f} (LB {got['lb']:+.2f}, SD {got['sd']:.2f}), passes kernel {p['kernel']} "
            f"workers {p['workers']} solo {p['solo']}, mean kernel {float(m['kernel']):.2f} "
            f"workers {float(m['workers']):.2f} solo {float(m['solo']):.2f}")


def decide(tables):
    """(word, text) from {family: {arm: [trial rows]}}. K3.1 decides first: flash is read only once
    K3.1 has cleared, and a K3.1 that cannot be judged leaves the verdict open."""
    got = {family: judge(tables[family]) for family in FAMILIES if tables.get(family)}
    text = "; ".join(describe(family, got[family]) for family in FAMILIES if family in got)
    if "kimi" not in got:
        return "UNRESOLVED", "; ".join(filter(None, [text, "no Kimi trial ran"]))
    if got["kimi"]["open"]:
        return "UNRESOLVED", text
    if not clears("kimi", got["kimi"]):
        return "KILL", text
    if "glm-flash" not in got or got["glm-flash"]["open"]:
        return "UNRESOLVED", text + "; K3.1 cleared and flash is not settled"
    return ("BUILD" if clears("glm-flash", got["glm-flash"]) else "KILL"), text


def verdict(rows):
    tables = {}
    for row in {row["run"]: row for row in rows if row.get("run")}.values():
        tables.setdefault(row["family"], {}).setdefault(row["run"].rsplit("-", 1)[1], []).extend(trials_of(row["run"]))
    return "%s: %s" % decide(tables)


def dataset():
    """The digest every sweep runs, read from the driver that owns it."""
    return re.search(r'^DATASET="([^"]+)"', (EVALS / "drivers" / "tbv4_baseline.sh").read_text(), re.M)[1]


def precheck(out):
    """Oracle and nop once per task, one job at a time, spending nothing; a job that already holds a
    result is read, not run again. Prints each trial's count, its source and its host, and exits 1 if
    a verifier scores in fewer than MIN_UNITS steps, an oracle does not pass or a nop does."""
    bad = 0
    for agent in ("nop", "oracle"):
        for task in TASKS:
            job = out / f"{agent}-{task}"
            if not any(job.rglob("result.json")):
                subprocess.run(["harbor", "run", "--agent", agent, "-d", dataset(), "-i", f"terminal-bench/{task}",
                                "-k", "1", "-n", "1", "--yes", "-q", "--agent-timeout-multiplier", "0.125",
                                "--verifier-timeout-multiplier", "1.5", "-o", str(job)], check=True)
            for result in sorted(job.rglob("result.json")):
                if not (result.parent.parent / "result.json").is_file():
                    continue
                context, got = axes.harbor_context(result.parent), tally(result.parent)
                print(json.dumps({"kind": "precheck", "agent": agent, "task": task, "tally": got, "errored": context["errored"],
                                  "reward": context["reward"], "host": platform.machine()}))
                bad += got is None or got[1] < MIN_UNITS or (context["reward"] >= 1) != (agent == "oracle")
    return 1 if bad else 0


def main(argv):
    if argv[:1] == ["precheck"]:
        out = Path(argv[1]) if argv[1:] else Path(tempfile.mkdtemp(prefix="s5-kc-ab-precheck-"))
        return precheck(out)
    if argv[:1] == ["table"]:
        found = argv[1:] or sorted(p.name for p in runs_root().glob("ns-s5-kc-ab-*"))
        for run in found:
            for row in trials_of(run):
                score = None if row["score"] is None else float(row["score"])
                print(json.dumps({**row, "score": score}, sort_keys=True))
        return 0
    print(__doc__, file=sys.stderr)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
