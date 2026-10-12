"""Paired gate for history search (#1184): a fact printed only in a tool result, two tasks.

long-parent: a parent runs report.py, then a child must say what it printed. past-output: an
earlier session ran report.py, a new one must say what it printed. The value is random per
run, so a re-run answers wrong; each run is graded on the acting agent's own final text
against what the seeding tool result printed. A seed whose reply text repeats the value is
re-seeded, since compact.search indexes replies and would leak it.

    python3 evals/search_gate.py --arm main=/path/yi --arm step1=/path/yi -k 6 --out /scratch

Runs real `yi ask` against the model (GLM 5.3 flash by default), so it spends money: about
$0.01 a run. Sessions land in <out>/sessions through `--session-dir`, never ~/.yi/sessions.
"""
import argparse, json, os, pathlib, re, subprocess, time

SCRIPT = '''import secrets
for i in range(300):
    print(f"setting_{i:03d} = {secrets.token_hex(3)}")
    if i == 217:
        print(f"release_id = rel-{secrets.token_hex(4)}")
'''
LONG_PARENT = (
    "Run `python3 report.py` once with bash. Do not repeat its output to me. Then spawn exactly "
    "one child in ipython with rlm.run(role=\"root\") whose prompt is only: 'Your parent ran "
    "report.py earlier; its output changes every run. What release_id did your parent's run "
    "print? Reply with the value alone.' Do not tell the child the value and do not use fork. "
    "Wait for the child and reply with exactly what it answered."
)
PAST_SEED = "Run `python3 report.py` once with bash and tell me only whether it succeeded."
PAST_ASK = (
    "An earlier session in this repository ran report.py; its output changes every run. What "
    "release_id did that earlier run print? Reply with the value alone."
)
RELEASE = re.compile(r"rel-[0-9a-f]{8}")


def project(out, name):
    proj = out / name
    proj.mkdir(parents=True)
    (proj / "README.md").write_text("# demo service\n")
    (proj / "report.py").write_text(SCRIPT)
    for args in (["init", "-q"], ["add", "-A"],
                 ["-c", "user.email=e@x", "-c", "user.name=e", "commit", "-qm", "init"]):
        subprocess.run(["git", *args], cwd=proj, check=True, capture_output=True)
    return proj


def ask(binary, model, sessions, proj, prompt):
    """One `yi ask` whose store is `sessions`; its root session file is the one it added."""
    before = set(sessions.glob("*/*.jsonl"))
    started = time.monotonic()
    done = subprocess.run([binary, "ask", "--model", model, "--session-dir", str(sessions), prompt],
                          cwd=proj, capture_output=True, text=True, timeout=900)
    new = sorted(set(sessions.glob("*/*.jsonl")) - before, key=os.path.getmtime)
    return (new[-1] if new else None), time.monotonic() - started, done.returncode


def messages(path):
    for line in open(path):
        message = json.loads(line).get("message")
        if message:
            yield message


def printed(path):
    """The release_id the session's first matching tool result printed: the ground truth."""
    for message in messages(path):
        if message.get("role") == "toolResult":
            found = RELEASE.search(json.dumps(message.get("content")))
            if found:
                return found.group(0)
    return None


def blocks(path, kind):
    for message in messages(path):
        if message.get("role") == "assistant":
            for block in message.get("content") or []:
                if isinstance(block, dict) and block.get("type") == kind:
                    yield message, block


def echoed(path):
    return any(RELEASE.search(block.get("text", "")) for _, block in blocks(path, "text"))


def tally(files):
    calls = history_greps = 0
    cost = 0.0
    for path in files:
        for message in messages(path):
            if message.get("role") == "assistant":
                cost += float(((message.get("usage") or {}).get("cost") or {}).get("total") or 0)
        for _, block in blocks(path, "toolCall"):
            calls += 1
            path_arg = str((block.get("arguments") or {}).get("path", ""))
            history_greps += block.get("name") == "grep" and path_arg.startswith("history://")
    return calls, history_greps, cost


def final_text(path):
    last = ""
    for _, block in blocks(path, "text"):
        last = block.get("text", "").strip() or last
    return last


def run(args, task, rep, arm):
    binary = args.arms[arm]
    proj = project(args.out, f"{task}-{arm}-{rep}-{time.time_ns()}")
    if task == "long-parent":
        session, seconds, code = ask(binary, args.model, args.sessions, proj, LONG_PARENT)
        truth = printed(session) if session else None
        children = sorted(session.with_suffix("").glob("children/*/*.jsonl")) if session else []
        actor, files = (children[-1] if children else None), ([session] + children if session else [])
    else:
        seed = None
        for _ in range(4):
            seed, _, _ = ask(args.arms[args.seed_arm], args.model, args.sessions, proj, PAST_SEED)
            if seed and not echoed(seed):
                break
            proj = project(args.out, f"{task}-{arm}-{rep}-{time.time_ns()}")
        truth = printed(seed) if seed and not echoed(seed) else None
        session, seconds, code = ask(binary, args.model, args.sessions, proj, PAST_ASK)
        actor, files = session, ([session] if session else [])
    calls, history_greps, cost = tally(files)
    answer = final_text(actor) if actor else ""
    row = {"arm": arm, "task": task, "rep": rep, "truth": truth, "answer": answer[:80],
           "pass": bool(truth) and truth in answer, "calls": calls,
           "history_greps": history_greps, "cost": round(cost, 5),
           "seconds": round(seconds, 1), "exit": code, "session": str(session) if session else None}
    print(json.dumps(row), flush=True)
    return row


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--arm", action="append", required=True, help="name=path/to/yi")
    parser.add_argument("--seed-arm", help="the arm whose binary seeds past-output (default: first)")
    parser.add_argument("-k", type=int, default=3)
    parser.add_argument("--out", type=pathlib.Path, required=True)
    parser.add_argument("--model", default="openrouter/z-ai/glm-5.3-flash")
    parser.add_argument("tasks", nargs="*", default=["long-parent", "past-output"])
    args = parser.parse_args()
    args.arms = dict(arm.split("=", 1) for arm in args.arm)
    args.seed_arm = args.seed_arm or next(iter(args.arms))
    args.sessions = args.out / "sessions"
    args.sessions.mkdir(parents=True, exist_ok=True)
    rows = [run(args, task, rep, arm)
            for rep in range(args.k) for task in args.tasks for arm in args.arms]
    with open(args.out / "rows.jsonl", "a") as out:
        out.writelines(json.dumps(row) + "\n" for row in rows)
    for task in args.tasks:
        for arm in args.arms:
            sub = [row for row in rows if row["task"] == task and row["arm"] == arm]
            n = len(sub)
            print(f"{task:12s} {arm:8s} pass {sum(r['pass'] for r in sub)}/{n}"
                  f"  calls {sum(r['calls'] for r in sub) / n:.1f}"
                  f"  history greps {sum(r['history_greps'] for r in sub)}"
                  f"  cost ${sum(r['cost'] for r in sub) / n:.4f}")


if __name__ == "__main__":
    main()
