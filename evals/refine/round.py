#!/usr/bin/env python3
"""One refine round (docs/plans/2026-09-26-self-improvement-evals.md 5.3-5.5): propose, then judge.

    python3 evals/refine/round.py propose --base <sha> --binary <musl yi> --out DIR [--n 3]
    python3 evals/refine/round.py round --base <sha> --binary <yi> --usd D --variant DIR|PATCH ... [--dry]

The owner starts a round; nothing schedules one. The proposer is `yi ask` in a container that
mounts exactly two things: a snapshot of the base (no git history, no eval ledger, no trial
store, all of which carry validation rows) and a corpus of development-task rows and sessions.
Leakage is that mount rule, not advice in the brief. Each run leaves one candidate: a patch
against the snapshot plus `.candidate.json` ({levers, target, rationale}). S0 refuses a patch
outside the lever surface, one naming a held-out task, and one a verdict row already rejected on
this base; the rationale is prose about the change and never part of its identity.

`round` takes proposed variants (a `propose` output directory), given patches, and the queue that
`/learn` writes, and walks each through one cascade: S0 ($0), a code variant's gate (build and
`just check` with no secret in its environment), the economy screen, the capability validation on
fresh seeds, and a Terminal-Bench transfer check at k=1. Every stage's verdict row lands in the
trial store, so S0 refuses a variant a stage already rejected on this base, and only a variant that
passes every stage is kept: a branch with its patch, which the owner opens as a draft PR and merges.
`--usd` is required; a stage whose predicted spend the round has not got left is not run.
"""
import argparse, hashlib, json, os, pathlib, shutil, subprocess, sys, tarfile, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "evals" / "graph"))
sys.path.insert(0, str(ROOT / "evals" / "adapters"))
from refine import names  # noqa: E402
from yi_usage import parse_events  # noqa: E402

STORE = ROOT / "evals" / "trials"
SPLIT = ROOT / "evals" / "levers" / "split.json"
BRIEF = pathlib.Path(__file__).resolve().parent / "brief.md"
# The lever surface a candidate may touch: prompt assets, tool descriptions, and the locks
# those move (each lock update rides as its own ratchet commit ahead of the code, D-row law).
SURFACE = ("crates/runtime/src/prompts/", "crates/tools/src/hashline/prompt.md", "crates/tools/src/builtins.rs",
           "crates/tools/src/grep.rs", "crates/tools/src/orient.rs", "crates/runtime/src/todo/text.rs",
           "scripts/guardrails/baselines/tool_surface.json", "scripts/guardrails/baselines/request_budget.json")
# A code variant (owner's decision: "Also code") may touch crate sources, never yi-types, whose
# shapes and fixtures stay human-owned, and only through the gate below.
CODE_SURFACE = ("crates/",)
CODE_EXCLUDED = ("crates/types/",)
GATE = os.environ.get("REFINE_GATE", "just check")
QUEUE = pathlib.Path.home() / ".yi" / "refine" / "queue.jsonl"
# The environment a variant's build and gate run with: no key, token or credential reaches code a
# model wrote; HOME stays so cargo and uv find their caches.
KEPT_ENV = ("PATH", "HOME", "LANG", "TERM", "CARGO_HOME", "RUSTUP_HOME", "TMPDIR")
WITHHELD = ("docs/eval-ledger.md", "evals/trials")
IMAGE = "python:3.12-slim"


def held_out(split):
    return split["validation"] + split["final"]


def task_of(text):
    return text.rsplit("/", 1)[-1]


def corpus(split, store, runs, out):
    """Copy the development group's trial rows and session files into `out`; nothing else."""
    out, held, dev = pathlib.Path(out), held_out(split), set(split["development"])
    (out / "sessions").mkdir(parents=True, exist_ok=True)
    kept, withheld = [], 0
    for path in sorted(pathlib.Path(store).glob("*.jsonl")):
        for line in path.read_text().splitlines():
            if not line.strip():
                continue
            row = json.loads(line)
            if task_of(str(row.get("task", ""))) in dev and not names(line, held):
                kept.append(line)
            else:
                withheld += 1
    (out / "rows.jsonl").write_text("".join(line + "\n" for line in kept))
    sessions = 0
    for result in sorted(pathlib.Path(runs).rglob("result.json")):
        trial = result.parent
        files = sorted((trial / "agent" / "yi" / "sessions").glob("*.jsonl")) if (trial / "agent").is_dir() else []
        if not files:
            continue
        task = task_of(json.loads(result.read_text()).get("task_name") or "")
        texts = [f.read_text(errors="replace") for f in files]
        if task not in dev or any(names(json.dumps(text), held) for text in texts):
            withheld += 1
            continue
        target = out / "sessions" / trial.name
        target.mkdir(parents=True, exist_ok=True)
        for file, text in zip(files, texts):
            (target / file.name).write_text(text)
        sessions += 1
    return {"rows": len(kept), "sessions": sessions, "withheld": withheld}


def snapshot(repo, sha, out):
    """The base tree as files: `git archive`, so no history, minus the ledger and the trial store."""
    out = pathlib.Path(out)
    out.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryFile() as archive:
        subprocess.run(["git", "archive", "--format=tar", sha], cwd=repo, stdout=archive, check=True)
        archive.seek(0)
        with tarfile.open(fileobj=archive) as tar:
            tar.extractall(out, filter="data")
    for relative in WITHHELD:
        target = out / relative
        if target.is_dir():
            shutil.rmtree(target)
        elif target.exists():
            target.unlink()
    return out


def touched(patch):
    return [line.split(" b/", 1)[1] for line in patch.splitlines() if line.startswith("diff --git a/")]


def is_code(patch):
    return any(path.startswith(CODE_SURFACE) and not path.startswith(SURFACE) for path in touched(patch))


def s0(patch, levers, split, code=False):
    """The reason a candidate is refused before anything is paid for, or None."""
    if not patch.strip() and not levers:
        return "no_change"
    for path in touched(patch):
        allowed = path.startswith(SURFACE) or (code and path.startswith(CODE_SURFACE) and not path.startswith(CODE_EXCLUDED))
        if not allowed:
            return f"path_outside_surface:{path}"
    named = names(json.dumps(patch), held_out(split)) or names(json.dumps(levers), held_out(split))
    if named:
        return f"names_held_out:{named[0]}"
    return None


def candidate_hash(patch, levers):
    body = patch + "\n" + json.dumps(levers, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(body.encode()).hexdigest()[:16]


def rejected(candidate, base, store=STORE):
    """The reason a verdict row on this base already rejected the candidate, or None."""
    for path in sorted(pathlib.Path(store).glob("*.jsonl")):
        for line in path.read_text().splitlines():
            row = json.loads(line) if line.strip() else {}
            if row.get("kind") == "verdict" and row.get("candidate") == candidate and row.get("base") == base \
                    and row.get("verdict") != "better":
                return row.get("reason") or row.get("verdict")
    return None


def propose(base, binary, out, n, model, runs):
    """`n` independent proposer runs, each in a fresh snapshot; one candidate directory each."""
    out = pathlib.Path(out).resolve()
    split = json.loads(SPLIT.read_text())
    counted = corpus(split, STORE, runs, out / "corpus")
    print(f"corpus: {counted}", file=sys.stderr)
    for index in range(1, n + 1):
        work = snapshot(ROOT, base, out / f"c{index}" / "work")
        pristine = snapshot(ROOT, base, out / f"c{index}" / "base")
        home = out / f"c{index}" / "home" / ".yi"
        home.mkdir(parents=True)
        (home / "config.json").write_text(json.dumps({"kernel": {"prewarm": False}}))
        command = ["docker", "run", "--rm", "-e", "OPENROUTER_API_KEY", "-e", "HOME=/home/p",
                   "-v", f"{work}:/work", "-v", f"{out / 'corpus'}:/corpus:ro",
                   "-v", f"{home.parent}:/home/p", "-v", f"{pathlib.Path(binary).resolve()}:/usr/local/bin/yi:ro",
                   "-w", "/work", IMAGE, "yi", "ask", "--json", "--yolo", "--here", "--model", model,
                   "--session-dir", "/home/p/sessions", BRIEF.read_text()]
        with (out / f"c{index}" / "events.jsonl").open("w") as sink:
            subprocess.run(command, stdout=sink, stderr=subprocess.STDOUT, env=os.environ, check=False)
        spec_path = work / ".candidate.json"
        spec = json.loads(spec_path.read_text()) if spec_path.is_file() else {}
        spec_path.unlink(missing_ok=True)
        patch = subprocess.run(["git", "diff", "--no-index", "--", "base", "work"], cwd=out / f"c{index}",
                               capture_output=True, text=True).stdout
        patch = patch.replace("a/base/", "a/").replace("b/work/", "b/")
        levers = spec.get("levers") or {}
        reason = s0(patch, levers, split)
        candidate = candidate_hash(patch, levers)
        reason = reason or (f"already_rejected:{rejected(candidate, base)}" if rejected(candidate, base) else None)
        (out / f"c{index}" / "candidate.patch").write_text(patch)
        # The proposer's own spend is development cost the verdict's break-even must cover.
        spent = parse_events(out / f"c{index}" / "events.jsonl").get("costUsd")
        verdict = {"kind": "s0", "candidate": candidate, "base": base, "levers": levers,
                   "target": spec.get("target"), "refused": reason, "proposerUsd": spent}
        (out / f"c{index}" / "s0.json").write_text(json.dumps(verdict, indent=1) + "\n")
        print(json.dumps(verdict))
        shutil.rmtree(pristine)

STAGES = (("screen", "inner-economy", "development"), ("validate", "inner-capability", "validation"),
          ("transfer", "tbv4", "validation"))


def scrubbed_env():
    return {key: os.environ[key] for key in KEPT_ENV if key in os.environ}


def variants(sources, queue=QUEUE):
    """(name, patch, levers, target) from `propose` directories, patch files and queued rows."""
    out = []
    for source in sources:
        path = pathlib.Path(source)
        if path.is_dir():
            for directory in sorted(path.glob("c*")):
                spec = json.loads((directory / "s0.json").read_text()) if (directory / "s0.json").is_file() else {}
                patch = (directory / "candidate.patch").read_text() if (directory / "candidate.patch").is_file() else ""
                out.append((directory.name, patch, spec.get("levers") or {}, spec.get("target")))
        else:
            out.append((path.name, path.read_text(), {}, f"given: {path.name}"))
    if pathlib.Path(queue).is_file():
        for line in pathlib.Path(queue).read_text().splitlines():
            row = json.loads(line) if line.strip() else {}
            if row.get("patch"):
                out.append((f"queue-{len(out)}", row["patch"], row.get("levers") or {}, row.get("target")))
    return out


def build(base, patch, work, code):
    """A variant's binary: the base worktree with its patch, built, and gated when it is code."""
    subprocess.run(["git", "worktree", "add", "--detach", str(work), base], cwd=ROOT, check=True, capture_output=True)
    applied = subprocess.run(["git", "apply", "-"], cwd=work, input=patch, text=True, capture_output=True)
    if applied.returncode:
        return None, "patch_does_not_apply"
    env = {**scrubbed_env(), **({"CARGO_BUILD_TARGET": os.environ["CARGO_BUILD_TARGET"]}
                                if os.environ.get("CARGO_BUILD_TARGET") else {})}
    steps = [["cargo", "build", "--profile", "dist", "-p", "yi-cli"]] + ([["sh", "-c", GATE]] if code else [])
    for step in steps:
        if subprocess.run(step, cwd=work, env=env, capture_output=True).returncode:
            return None, "gate_red" if step[0] == "sh" else "build_red"
    triple = os.environ.get("CARGO_BUILD_TARGET")
    return work / "target" / (f"{triple}/dist" if triple else "dist") / "yi", None


def measure(stage, suite, group, base_binary, variant_binary, run_id):
    """One paired stage over its registry group: base and variant on the same tasks, k=1."""
    sys.path.insert(0, str(ROOT / "evals"))
    sys.path.insert(0, str(ROOT / "evals" / "drivers"))
    import weekly  # noqa: E402
    tasks = registry()["suites"][suite]["groups"].get(group, [])
    if registry()["suites"][suite]["harness"] == "inner":
        return [weekly.call(base_binary, "base", tasks, run_id)], [weekly.call(variant_binary, "variant", tasks, run_id)]
    # Harbor uploads the binary into each task's container, so on Linux both builds are the static
    # musl target (set CARGO_BUILD_TARGET=x86_64-unknown-linux-musl for the round).
    arms = []
    for arm, binary in (("base", base_binary), ("variant", variant_binary)):
        with tempfile.NamedTemporaryFile("w", suffix=".json") as overrides:
            overrides.write("{}")
            overrides.flush()
            env = {**os.environ, "EVAL_BINARY": str(binary), "EVAL_ARM": arm, "EVAL_RUN_ID": run_id}
            done = subprocess.run(["sh", str(ROOT / "evals/drivers/harbor.sh"), suite, "--runner", overrides.name, *tasks],
                                  cwd=ROOT, env=env, capture_output=True, text=True, check=False)
        arms.append([json.loads(line) for line in done.stdout.splitlines() if line.strip().startswith("{")])
    return [arms[0]], [arms[1]]


def registry():
    sys.path.insert(0, str(ROOT / "evals"))
    import suites  # noqa: E402
    return suites.load()


def judge_stage(stage, suite, base_runs, runs, accesses):
    sys.path.insert(0, str(ROOT / "evals"))
    import levers  # noqa: E402
    tasks = sorted({row["task"] for run in base_runs + runs for row in run})
    floors = {suite: {"pass_min": 0.0, "reward_min": 0.0, "tolerance": 0.0, "tasks": tasks}}
    if not tasks:
        return {"verdict": "inconclusive", "reason": "no_rows", "devSpendUsd": 0.0}
    if stage == "screen":
        reason = levers.screen(base_runs, runs, floors)
        verdict = levers.judge(base_runs, runs, floors)
        return dict(verdict, verdict="rejected" if reason else "better", reason=reason)
    verdict = levers.judge(base_runs, runs, floors, accesses=accesses)
    if stage == "transfer":
        # A transfer check guards against a regression on the long-horizon suite; it is not asked
        # to show a gain that twelve tasks cannot detect (section 19).
        return dict(verdict, verdict="rejected" if verdict["verdict"] == "rejected" else "better")
    return verdict


def run_round(found, base, binary, usd, split, store=STORE, dry=False, build_fn=build, measure_fn=measure,
              out=None, run_id="refine", accesses=4):
    """Each variant through S0, its gate, and the stages, until one fails or the money runs out."""
    left, report = usd, []
    prices = {suite: row.get("trialUsd", 0.0) for suite, row in registry()["suites"].items()}
    for name, patch, levers_set, target in found:
        code = is_code(patch)
        variant = candidate_hash(patch, levers_set)
        reason = s0(patch, levers_set, split, code=code)
        reason = reason or (f"already_rejected:{rejected(variant, base, store)}" if rejected(variant, base, store) else None)
        row = {"variant": variant, "name": name, "target": target, "code": code, "stages": []}
        report.append(row)
        if reason:
            row["refused"] = reason
            continue
        if dry:
            row["predictedUsd"] = round(sum(2 * len(registry()["suites"][suite]["groups"].get(group, [])) * prices[suite]
                                            for _, suite, group in STAGES), 2)
            continue
        work = pathlib.Path(out or tempfile.mkdtemp()) / f"work-{variant}"
        variant_binary, refused = build_fn(base, patch, work, code)
        if refused:
            row["refused"] = refused
            continue
        for stage, suite, group in STAGES:
            predicted = 2 * len(registry()["suites"][suite]["groups"].get(group, [])) * prices[suite]
            if predicted > left:
                row["stopped"] = f"{stage}: ${predicted:.2f} predicted, ${left:.2f} left of --usd"
                break
            base_runs, runs = measure_fn(stage, suite, group, binary, variant_binary, run_id)
            verdict = judge_stage(stage, suite, base_runs, runs, accesses)
            left -= verdict.get("devSpendUsd", 0.0)
            stored = {"kind": "verdict", "candidate": variant, "base": base, "stage": stage, "suite": suite,
                      **{k: verdict.get(k) for k in ("verdict", "reason", "road", "by", "unit", "devSpendUsd", "breakEvenTasks")}}
            pathlib.Path(store).mkdir(parents=True, exist_ok=True)
            with (pathlib.Path(store) / f"{run_id}.jsonl").open("a") as sink:
                sink.write(json.dumps(stored, sort_keys=True) + "\n")
            row["stages"].append(stored)
            if verdict["verdict"] != "better":
                break
        else:
            row["kept"] = f"refine/{variant}"
    return {"base": base, "usd": usd, "leftUsd": round(left, 6), "variants": report}


def keep(result, base):
    """A branch per kept variant, its patch committed on the base: the owner opens and merges it."""
    for row in result["variants"]:
        if row.get("kept"):
            work = pathlib.Path(tempfile.mkdtemp()) / "keep"
            subprocess.run(["git", "worktree", "add", "-b", row["kept"], str(work), base], cwd=ROOT, check=True,
                           capture_output=True)
            print(f"kept {row['variant']}: branch {row['kept']} at {work}; commit the patch there, then `just pr open`")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    verbs = parser.add_subparsers(dest="verb", required=True)
    go = verbs.add_parser("propose")
    go.add_argument("--base", required=True)
    go.add_argument("--binary", required=True, help="a linux musl yi, mounted read-only into the container")
    go.add_argument("--out", required=True)
    go.add_argument("--n", type=int, default=3)
    go.add_argument("--model", default="openrouter/z-ai/glm-5.3-flash")
    go.add_argument("--runs", default=str(pathlib.Path.home() / "Development" / "yi-runs"))
    judged = verbs.add_parser("round")
    judged.add_argument("--base", required=True)
    judged.add_argument("--binary", required=True, help="the base, built")
    judged.add_argument("--usd", type=float, required=True, help="the round's whole budget")
    judged.add_argument("--variant", action="append", default=[], help="a propose output dir or a patch file")
    judged.add_argument("--dry", action="store_true", help="S0 and predicted spend only")
    judged.add_argument("--run-id", default="refine-" + pathlib.Path(tempfile.mktemp()).name)
    args = parser.parse_args(argv)
    if args.verb == "round":
        if args.usd <= 0 and not args.dry:
            print("refused: --usd must be positive for a round that measures", file=sys.stderr)
            return 2
        split = json.loads(SPLIT.read_text())
        result = run_round(variants(args.variant), args.base, args.binary, args.usd, split, dry=args.dry,
                           run_id=args.run_id)
        print(json.dumps(result, indent=1, sort_keys=True))
        if not args.dry:
            keep(result, args.base)
        return 0 if any(row.get("kept") for row in result["variants"]) else 3
    if not os.environ.get("OPENROUTER_API_KEY"):
        print("refused: OPENROUTER_API_KEY is unset", file=sys.stderr)
        return 1
    if not 1 <= args.n <= 3:
        print("refused: a round takes one to three candidates", file=sys.stderr)
        return 1
    propose(args.base, args.binary, args.out, args.n, args.model, args.runs)
    return 0


if __name__ == "__main__":
    sys.exit(main())
