#!/usr/bin/env python3
"""One improvement round's proposer half (docs/plans/2026-09-26-self-improvement-evals.md 5.3-5.5).

    python3 evals/improve/round.py propose --base <sha> --binary <musl yi> --out DIR [--n 3]

The owner starts a round; nothing schedules one. The proposer is `yi ask` in a container that
mounts exactly two things: a snapshot of the base (no git history, no eval ledger, no trial
store, all of which carry validation rows) and a corpus of development-task rows and sessions.
Leakage is that mount rule, not advice in the brief. Each run leaves one candidate: a patch
against the snapshot plus `.candidate.json` ({levers, target, rationale}). S0 refuses a patch
outside the lever surface, one naming a held-out task, and one a verdict row already rejected on
this base; the rationale is prose about the change and never part of its identity.
"""
import argparse, hashlib, json, os, pathlib, shutil, subprocess, sys, tarfile, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "evals" / "graph"))
from refine import names  # noqa: E402

STORE = ROOT / "evals" / "trials"
SPLIT = ROOT / "evals" / "levers" / "split.json"
BRIEF = pathlib.Path(__file__).resolve().parent / "brief.md"
# The lever surface a candidate may touch: prompt assets, tool descriptions, and the locks
# those move (each lock update rides as its own ratchet commit ahead of the code, D-row law).
SURFACE = ("crates/runtime/src/prompts/", "crates/tools/src/hashline/prompt.md", "crates/tools/src/builtins.rs",
           "crates/tools/src/grep.rs", "crates/tools/src/orient.rs", "crates/runtime/src/todo/text.rs",
           "scripts/guardrails/baselines/tool_surface.json", "scripts/guardrails/baselines/request_budget.json")
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


def s0(patch, levers, split):
    """The reason a candidate is refused before anything is paid for, or None."""
    if not patch.strip() and not levers:
        return "no_change"
    for path in touched(patch):
        if not path.startswith(SURFACE):
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
        verdict = {"kind": "s0", "candidate": candidate, "base": base, "levers": levers,
                   "target": spec.get("target"), "refused": reason}
        (out / f"c{index}" / "s0.json").write_text(json.dumps(verdict, indent=1) + "\n")
        print(json.dumps(verdict))
        shutil.rmtree(pristine)


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
    args = parser.parse_args(argv)
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
