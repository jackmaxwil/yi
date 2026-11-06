#!/usr/bin/env python3
"""Which stored memories a session's first message earns, and what it costs (#1860).

    python3 evals/memory_inject.py measure [--sessions ~/.yi/sessions] [--json]
    python3 evals/memory_inject.py replay --blocks target/memory-inject/after.jsonl
    python3 evals/memory_inject.py label --model openrouter/deepseek/deepseek-v4.1 --cap-usd 10
    python3 evals/memory_inject.py check

`measure` is the census: it reads the memory fence each recorded session carries into its
first request and prints counts only, never a note's text.

`replay` is the eval, and it spends nothing: the labels under labels.json say which notes the
first message of each case is about, `before/block.txt` is what the pre-change build rendered
for the whole fixture store, and `--blocks` is what this build's `memory::focused` renders per
case (dumped by `crates/runtime/tests/memory_inject.rs`). It prints both arms and fails when
the focused block costs more tokens than the index it replaces, or earns fewer relevant bodies.

`label` is the paid half and is the owner's to run: one judge call per case naming the relevant
notes and a reason for each, refused past `--cap-usd` or when a reply has no countable cost. It
writes only judged cases to labels-judge.json, never over the hand labels. Its run is a ledger row.
"""

import argparse
import json
import math
import os
import re
import sys
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "evals" / "fixtures" / "memory-inject"
LABELS = FIXTURE / "labels.json"
BEFORE = FIXTURE / "before" / "block.txt"
# The floor the suite holds the focused block to, however the before arm scored (#1860).
FLOOR = {"recall_body": 0.5, "precision_body": 0.2}
ENDPOINT = "https://openrouter.ai/api/v1/chat/completions"
JUDGE = (
    "You are labelling a memory store for a coding agent. Given the agent's stored notes (name, "
    "type, hook) and the first message a person typed, reply with JSON "
    '{"relevant": [{"name": "<note name>", "why": "<one sentence>"}]} naming only the notes that '
    "carry a fact the agent needs to act on THAT message: its rule, its file, or its command. A "
    "note that merely shares a topic is not relevant. An empty list is a valid answer."
)
ROW = re.compile(r"^- \[([^\]]+)\]\([^)]*\.md\) — ")
BODY = re.compile(r"^## (\S+) — ")


def note_rows(block):
    """The notes a block names, and the ones whose body it carries."""
    names, bodies = [], []
    for line in block.splitlines():
        row = ROW.match(line)
        if row:
            names.append(row.group(1))
        head = BODY.match(line)
        if head:
            names.append(head.group(1))
            bodies.append(head.group(1))
    return names, bodies


def tokens(text):
    return math.ceil(len(text) / 4)


def score(block, case, aliases):
    names, bodies = note_rows(block)
    satisfied, satisfied_bodies, wrong, wrong_bodies = 0, 0, 0, 0
    for name in case["relevant"]:
        group = {name, *aliases.get(name, [])}
        satisfied += 1 if group & set(names) else 0
        satisfied_bodies += 1 if group & set(bodies) else 0
    labelled = set()
    for name in case["relevant"]:
        labelled.update({name, *aliases.get(name, [])})
    wrong = len([n for n in names if n not in labelled])
    wrong_bodies = len([n for n in bodies if n not in labelled])
    return {
        "shown": len(names),
        "bodies": len(bodies),
        "relevant_shown": satisfied,
        "relevant_bodies": satisfied_bodies,
        "wrong": wrong,
        "wrong_bodies": wrong_bodies,
        "bytes": len(block),
        "tokens": tokens(block),
        "labels": len(case["relevant"]),
    }


def ratio(part, whole):
    return part / whole if whole else 0.0


def arm(rows):
    labels = sum(row["labels"] for row in rows)
    return {
        "labels": labels,
        "relevant_shown": sum(row["relevant_shown"] for row in rows),
        "relevant_bodies": sum(row["relevant_bodies"] for row in rows),
        "shown": sum(row["shown"] for row in rows),
        "bodies": sum(row["bodies"] for row in rows),
        "wrong": sum(row["wrong"] for row in rows),
        "wrong_bodies": sum(row["wrong_bodies"] for row in rows),
        "tokens": sum(row["tokens"] for row in rows),
        "bytes": sum(row["bytes"] for row in rows),
    }


def table(name, summary):
    print(
        f"  {name:<8} labels {summary['labels']:>3} · recall@body "
        f"{ratio(summary['relevant_bodies'], summary['labels']):.2f} · precision@body "
        f"{ratio(summary['relevant_bodies'], summary['bodies']):.2f} · recall@shown "
        f"{ratio(summary['relevant_shown'], summary['labels']):.2f} · precision@shown "
        f"{ratio(summary['relevant_shown'], summary['shown']):.2f} · wrong {summary['wrong']} "
        f"({summary['wrong_bodies']} with a body) · {summary['tokens']} tokens"
    )


def load_blocks(path):
    blocks = {}
    for line in Path(path).read_text().splitlines():
        if not line.strip():
            continue
        record = json.loads(line)
        blocks[record["id"]] = record["block"]
    return blocks


def replay(args):
    labels = json.loads(LABELS.read_text())
    before_text = BEFORE.read_text()
    after = load_blocks(args.blocks)
    missing = [case["id"] for case in labels["cases"] if case["id"] not in after]
    if missing:
        print(f"FAIL replay: no focused block for {', '.join(missing)}", file=sys.stderr)
        return 1
    before_rows, after_rows = [], []
    print(f"{labels['suite']} · {len(labels['cases'])} cases · labels by {labels['labeling']['by']}")
    for case in labels["cases"]:
        aliases = case.get("aliases") or {}
        old = score(before_text, case, aliases)
        new = score(after[case["id"]], case, aliases)
        before_rows.append(old)
        after_rows.append(new)
        print(
            f"  {case['id']:<18} before {old['tokens']:>4}t body {old['relevant_bodies']} · "
            f"after {new['tokens']:>4}t body {new['relevant_bodies']} · "
            f"wrong {new['wrong']} · {'ok' if new['tokens'] <= old['tokens'] else 'OVER BUDGET'}"
        )
    old, new = arm(before_rows), arm(after_rows)
    table("before", old)
    table("after", new)
    problems = []
    for case, old_row, new_row in zip(labels["cases"], before_rows, after_rows):
        if new_row["tokens"] > old_row["tokens"]:
            problems.append(
                f"{case['id']}: {new_row['tokens']} tokens against {old_row['tokens']} before"
            )
    if new["tokens"] > old["tokens"]:
        problems.append(f"tokens {new['tokens']} > {old['tokens']} before")
    if new["relevant_bodies"] < old["relevant_bodies"]:
        problems.append(f"recall@body {new['relevant_bodies']} < {old['relevant_bodies']} before")
    recall = ratio(new["relevant_bodies"], new["labels"])
    precision = ratio(new["relevant_bodies"], new["bodies"])
    if recall < FLOOR["recall_body"]:
        problems.append(f"recall@body {recall:.2f} is under the {FLOOR['recall_body']:.2f} floor")
    if precision < FLOOR["precision_body"]:
        problems.append(
            f"precision@body {precision:.2f} is under the {FLOOR['precision_body']:.2f} floor"
        )
    if new["wrong"] > old["wrong"]:
        problems.append(f"{new['wrong']} irrelevant notes named, {old['wrong']} before")
    for problem in problems:
        print(f"FAIL replay: {problem}", file=sys.stderr)
    if problems:
        return 1
    print(
        f"PASS replay: recall@body {ratio(new['relevant_bodies'], new['labels']):.2f} against "
        f"{ratio(old['relevant_bodies'], old['labels']):.2f}, {new['tokens']} tokens against "
        f"{old['tokens']}"
    )
    return 0


def check(_args):
    labels = json.loads(LABELS.read_text())
    notes = {path.stem for path in (FIXTURE / "home" / ".yi" / "memory").glob("*.md")}
    problems = []
    for case in labels["cases"]:
        if not case["relevant"]:
            problems.append(f"{case['id']} labels nothing")
        for name in case["relevant"]:
            if name not in notes:
                problems.append(f"{case['id']} labels {name}, which no note has")
        for name in case.get("aliases") or {}:
            if name not in notes:
                problems.append(f"{case['id']} aliases {name}, which no note has")
    if not BEFORE.exists():
        problems.append("before/block.txt is missing")
    for problem in problems:
        print(f"FAIL check: {problem}", file=sys.stderr)
    if problems:
        return 1
    print(f"PASS check: {len(labels['cases'])} cases, {len(notes)} notes, every label has a note")
    return 0


def census(path):
    """Per session: the memory fence's bytes and tokens, from the yard the session recorded."""
    rows = []
    for file in sorted(Path(path).expanduser().glob("**/*.jsonl")):
        try:
            lines = file.open(errors="replace")
        except OSError:
            continue
        with lines as handle:
            for line in handle:
                if "ext_state2" not in line:
                    continue
                try:
                    record = json.loads(line)
                except ValueError:
                    break
                yard = (record.get("data") or {}).get("yard") or []
                for entry in yard if isinstance(yard, list) else yard.get("entries", []):
                    if entry.get("source") == "memory":
                        text = entry.get("text", "")
                        rows.append((file, len(text), text.count("]("), tokens(text)))
                break
    return rows


def measure(args):
    rows = census(args.sessions)
    if not rows:
        print(f"no session under {args.sessions} carries a memory fence", file=sys.stderr)
        return 1
    sizes = sorted(row[1] for row in rows)
    counts = sorted(row[2] for row in rows)
    print(
        f"{len(rows)} sessions carry a memory fence · bytes min {sizes[0]} median "
        f"{sizes[len(sizes) // 2]} max {sizes[-1]} · notes min {counts[0]} median "
        f"{counts[len(counts) // 2]} max {counts[-1]} · tokens median "
        f"{tokens('x' * sizes[len(sizes) // 2])}"
    )
    if args.json:
        print(json.dumps([{"session": str(f), "bytes": b, "notes": n, "tokens": t} for f, b, n, t in rows]))
    return 0


def label(args):
    key = os.environ.get("OPENROUTER_API_KEY")
    if not key:
        print("FAIL label: OPENROUTER_API_KEY is not set; this half is the owner's", file=sys.stderr)
        return 1
    if not args.model.startswith("openrouter/"):
        print("FAIL label: --model must name an openrouter/ model", file=sys.stderr)
        return 1
    labels = json.loads(LABELS.read_text())
    store = FIXTURE / "home" / ".yi" / "memory"
    catalogue = []
    for path in sorted(store.glob("*.md")):
        head = path.read_text().split("---")
        fields = dict(
            line.split(":", 1) for line in head[1].splitlines() if ":" in line
        )
        catalogue.append(
            f"- {path.stem} ({fields.get('type', '?').strip()}): {fields.get('description', '').strip()}"
        )
    spent = 0.0
    judged = []
    for case in labels["cases"]:
        if spent >= args.cap_usd:
            print(f"stopped at the ${args.cap_usd} ceiling before {case['id']}; unjudged cases are omitted")
            break
        body = {
            "model": args.model.split("/", 1)[1],
            "messages": [
                {"role": "system", "content": JUDGE},
                {"role": "user", "content": "\n".join(catalogue) + "\n\nFirst message:\n" + case["prompt"]},
            ],
            "response_format": {"type": "json_object"},
            "usage": {"include": True},
        }
        request = urllib.request.Request(
            ENDPOINT,
            data=json.dumps(body).encode(),
            headers={"Authorization": f"Bearer {key}", "Content-Type": "application/json"},
        )
        with urllib.request.urlopen(request, timeout=120) as reply:
            answer = json.loads(reply.read())
        cost = answer.get("usage", {}).get("cost")
        if not isinstance(cost, (int, float)):
            print("FAIL label: refused: the reply carried no usage.cost, so --cap-usd cannot count it", file=sys.stderr)
            return 1
        spent += float(cost)
        said = json.loads(answer["choices"][0]["message"]["content"])
        case = dict(case)
        case["relevant"] = {row["name"]: row["why"] for row in said.get("relevant", [])}
        case["judge"] = args.model
        judged.append(case)
    labels["cases"] = judged
    labels["labeling"] = {
        "by": f"judge {args.model}",
        "date": __import__("datetime").date.today().isoformat(),
        "spent_usd": round(spent, 4),
    }
    (FIXTURE / "labels-judge.json").write_text(json.dumps(labels, indent=2) + "\n")
    print(f"labelled {len(judged)} cases for ${spent:.4f}; hand-check a sample before citing it")
    return 0


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    subs = parser.add_subparsers(dest="verb", required=True)
    census_ = subs.add_parser("measure", help="count what a session's first request carries")
    census_.add_argument("--sessions", default="~/.yi/sessions")
    census_.add_argument("--json", action="store_true")
    census_.set_defaults(run=measure)
    play = subs.add_parser("replay", help="score both arms against the saved labels, spending nothing")
    play.add_argument("--blocks", required=True, help="JSONL of {id, block} this build rendered")
    play.set_defaults(run=replay)
    paid = subs.add_parser("label", help="the paid judge run: one call a case, capped")
    paid.add_argument("--model", required=True)
    paid.add_argument("--cap-usd", type=float, default=10.0)
    paid.set_defaults(run=label)
    check_ = subs.add_parser("check", help="the labels name notes the store has")
    check_.set_defaults(run=check)
    args = parser.parse_args(argv)
    return args.run(args)


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
