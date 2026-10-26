#!/usr/bin/env python3
"""Paired root/reader children over 25 cited questions, with an exact paired kill verdict."""
from __future__ import annotations

import argparse
import ast
import asyncio
import json
import math
import sys
import subprocess
from collections import defaultdict
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "evals/fixtures/reader_vs_root/questions.json"
TRIALS = ROOT / "evals/trials/ns-b1-exp-d.jsonl"
sys.path.insert(0, str(ROOT / "python/yi_runtime/src"))
sys.path.insert(0, str(ROOT / "evals/adapters"))
import rlm  # noqa: E402
import yi_usage  # noqa: E402
from yi.roles import verify_quotes  # noqa: E402

ALPHA = 0.025
ARMS = ("root", "reader")
SCHEMA = {
    "type": "object",
    "required": ["answer", "quotes"],
    "properties": {
        "answer": {"type": "string"},
        "quotes": {
            "type": "array",
            "items": {
                "type": "object",
                "required": ["url", "line", "text"],
                "properties": {
                    "url": {"type": "string"},
                    "line": {"type": "integer"},
                    "text": {"type": "string"},
                },
            },
        },
    },
}
_CACHE: dict[str, str] = {}


def fixture():
    return json.loads(FIXTURE.read_text())


def source(commit: str, url: str) -> str:
    path = url.removeprefix("local://")
    key = f"{commit}:{path}"
    if key not in _CACHE:
        _CACHE[key] = subprocess.check_output(
            ["git", "-C", str(ROOT), "show", key], text=True
        )
    return _CACHE[key]


def validate_questions(document=None):
    document = document or fixture()
    questions = document["questions"]
    assert len(questions) == 25, f"expected 25 questions, got {len(questions)}"
    assert len({q["id"] for q in questions}) == 25, "question ids must be unique"
    for question in questions:
        page = source(document["commit"], question["url"])
        lines = page.splitlines()
        assert 0 < question["line"] <= len(lines), question["id"]
        assert question["quote"] in lines[question["line"] - 1], question["id"]
        assert question["answer"].strip(), question["id"]
    return document


def child_prompt(question):
    return (
        f"{question['question']}\n"
        "Answer from the pinned Yi source, not memory. Give the concise exact value or name. "
        "Cite the source line that decides it, copying its text exactly. Return only this JSON "
        'object: {"answer":"...","quotes":[{"url":"local://path","line":1,"text":"..."}]}. '
        f"The source address available to you is {question['url']}."
    )


def run_specs():
    document = validate_questions()
    specs = []
    for question in document["questions"]:
        for arm in ARMS:
            prompt = child_prompt(question)
            partition = [question["url"]]
            extra = {"turns": 5, "schema": SCHEMA} if arm == "reader" else {}
            code = (
                "import json\n"
                f"role = {arm!r}\n"
                f"child_prompt = {prompt!r}\n"
                f"partition = {partition!r}\n"
                f"extra = {extra!r}\n"
                "handle = await rlm.run(child_prompt, role=role, partition=partition, **extra)\n"
                "try:\n"
                "    result = await handle.result(timeout=540)\n"
                "    payload = result.get('json')\n"
                "    if payload is None:\n"
                "        payload = json.loads(result['text'])\n"
                "except Exception as error:\n"
                "    payload = {'answer': '', 'quotes': [], 'proseRefusal': True,\n"
                "               'error': f'{type(error).__name__}: {error}'}\n"
                "finally:\n"
                "    await rlm.delete_subagent(handle)\n"
                "print(json.dumps(payload, sort_keys=True))\n"
            )
            outer = (
                "Run exactly one child with the ipython code below. Do not answer the question "
                "yourself. After the cell finishes, your entire final response must be the JSON "
                "object it printed, with no fence or prose.\n\n" + code
            )
            specs.append({
                "id": f"{question['id']}-{arm}",
                "prompt": outer,
                "deadline": 600,
                "repo": str(ROOT),
                "rev": document["commit"],
            })
    return specs


def normalized(value):
    return " ".join(str(value).casefold().split())


async def score_answer(question, answer, commit):
    if not isinstance(answer, dict):
        return {"correct": False, "proseRefusal": True, "acceptedQuotes": []}
    kept = await verify_quotes(answer.get("quotes"), within=[question["url"]])
    accepted = [q for q in kept if q.get("url") == question["url"] and q.get("line") == question["line"]]
    correct = normalized(answer.get("answer", "")) == normalized(question["answer"]) and bool(accepted)
    return {"correct": correct, "proseRefusal": bool(answer.get("proseRefusal")), "acceptedQuotes": accepted}


def final_answer(row):
    path = Path(row.get("events") or "")
    if not path.is_absolute():
        path = runs_root() / "b1-exp-d" / row.get("run", "") / "events.jsonl"
    events, _ = yi_usage.json_lines(path)
    text = ""
    for event in events:
        if event.get("type") != "message_end":
            continue
        message = event.get("message") or {}
        if message.get("role") != "assistant":
            continue
        blocks = message.get("content") or []
        text = "\n".join(b.get("text", "") for b in blocks if isinstance(b, dict) and b.get("type") == "text")
    try:
        return json.loads(text)
    except (TypeError, ValueError):
        return None


def runs_root():
    import os
    return Path(os.environ.get("EVAL_KILL_ROOT") or Path.home() / "Development/yi-runs")


def child_usage(row):
    base = runs_root() / "b1-exp-d" / row.get("run", "") / "sessions"
    totals = {key: 0 for key in yi_usage.TOKEN_KEYS}
    files = [p for p in base.rglob("*.jsonl") if "sub-" in str(p)] if base.is_dir() else []
    messages = 0
    for path in files:
        entries, _ = yi_usage.json_lines(path)
        for entry in entries:
            message = entry.get("message") if entry.get("kind") == "entry" else None
            if not isinstance(message, dict) or message.get("role") != "assistant":
                continue
            messages += 1
            yi_usage._add_tokens(totals, message.get("usage"))
    return {**totals, "assistantMessages": messages, "sessionFiles": len(files)}


def run_key(row):
    if row.get("id"):
        return row["id"]
    name = row.get("run", "")
    for family in ("kimi", "glm-flash"):
        prefix = family + "-"
        if name.startswith(prefix) and len(name) > len(prefix) + 9:
            return name[len(prefix):-9]
    return name


def latest_pairs(rows):
    latest = {}
    for row in rows:
        key = run_key(row)
        question, sep, arm = key.rpartition("-")
        if sep and question and arm in ARMS:
            latest[(row["family"], question, arm)] = row
    return latest


def sign_p_value(improved, regressed):
    trials = improved + regressed
    return sum(math.comb(trials, k) for k in range(improved + 1)) / (2 ** trials) if trials else 1.0


def family_verdict(scored):
    by_question = defaultdict(dict)
    for row in scored:
        by_question[row["question"]][row["arm"]] = row
    complete = [pair for pair in by_question.values() if set(pair) == set(ARMS)]
    improved = sum(pair["reader"]["correct"] and not pair["root"]["correct"] for pair in complete)
    regressed = sum(pair["root"]["correct"] and not pair["reader"]["correct"] for pair in complete)
    root_correct = sum(pair["root"]["correct"] for pair in complete)
    reader_correct = sum(pair["reader"]["correct"] for pair in complete)
    prose_refusals = sum(row["proseRefusal"] for row in scored if row["arm"] == "reader")
    p_value = sign_p_value(improved, regressed)
    return {
        "questions": len(complete),
        "rootCorrect": root_correct,
        "readerCorrect": reader_correct,
        "improved": improved,
        "regressed": regressed,
        "signP": p_value,
        "readerProseRefusals": prose_refusals,
        "kill": len(complete) == 25 and (p_value <= ALPHA or root_correct - reader_correct > 3),
        "complete": len(complete) == 25,
    }


def report(rows):
    document = validate_questions()
    questions = {q["id"]: q for q in document["questions"]}

    async def fetch(url, *, as_text=False, offset=None, limit=None):
        page = source(document["commit"], url)
        return page if as_text else {"text": page}

    scored = []
    with patch.object(rlm, "fetch", fetch):
        for (family, qid, arm), row in latest_pairs(rows).items():
            answer = final_answer(row)
            outcome = asyncio.run(score_answer(questions[qid], answer, document["commit"]))
            scored.append({
                "family": family,
                "question": qid,
                "arm": arm,
                "run": row.get("run"),
                "answer": answer,
                **outcome,
                "childTokens": child_usage(row),
            })
    families = {}
    for family in sorted({row["family"] for row in scored}):
        families[family] = family_verdict([row for row in scored if row["family"] == family])
    return {"commit": document["commit"], "families": families, "children": scored}


def summarize(rows):
    result = report(rows)
    verdicts = [v for v in result["families"].values() if v["complete"]]
    verdict = "KILL" if any(v["kill"] for v in verdicts) else "KEEP" if verdicts else "UNRESOLVED"
    details = "; ".join(
        f"{family}: root {v['rootCorrect']}, reader {v['readerCorrect']}, "
        f"discordant {v['improved']}/{v['regressed']}, p={v['signP']:.6g}, "
        f"prose refusals {v['readerProseRefusals']}, {v['questions']}/25"
        for family, v in result["families"].items()
    )
    return f"{verdict} ({details})" if details else "UNRESOLVED (no scored children)"


def selftest():
    document = validate_questions()

    async def fetch(url, *, as_text=False, offset=None, limit=None):
        page = source(document["commit"], url)
        return page if as_text else {"text": page}

    with patch.object(rlm, "fetch", fetch):
        for question in document["questions"]:
            right = {"answer": question["answer"], "quotes": [{
                "url": question["url"], "line": question["line"], "text": question["quote"],
            }]}
            accepted = asyncio.run(score_answer(question, right, document["commit"]))
            assert accepted["correct"], f"keyed answer rejected: {question['id']}"
            page = source(document["commit"], question["url"]).splitlines()
            neighbour_line = question["line"] + 1 if question["line"] < len(page) else question["line"] - 1
            neighbour = {"answer": question["answer"], "quotes": [{
                "url": question["url"], "line": neighbour_line, "text": page[neighbour_line - 1],
            }]}
            rejected = asyncio.run(score_answer(question, neighbour, document["commit"]))
            assert not rejected["correct"], f"neighbour citation must not score: {question['id']}"
            wrong = {**right, "answer": "not the keyed answer"}
            assert not asyncio.run(score_answer(question, wrong, document["commit"]))["correct"]
    assert sign_p_value(0, 5) == 0.03125
    assert sign_p_value(0, 6) == 0.015625
    synthetic = lambda root_wins: [
        {"question": f"q{i:02}", "arm": arm, "correct": arm == "root" or i > root_wins, "proseRefusal": False}
        for i in range(1, 26) for arm in ARMS
    ]
    assert not family_verdict(synthetic(3))["kill"], "a three-question deficit is inside the kill boundary"
    assert family_verdict(synthetic(4))["kill"], "a four-question deficit crosses the kill boundary"
    specs = run_specs()
    assert len(specs) == 50 and len({s["id"] for s in specs}) == 50
    assert all(s["rev"] == document["commit"] for s in specs)
    for spec in specs:
        program = spec["prompt"].split("\n\n", 1)[1]
        compile(program, spec["id"], "exec", flags=ast.PyCF_ALLOW_TOP_LEVEL_AWAIT)
    malformed = {"answer": "", "quotes": [], "proseRefusal": True}
    outcome = asyncio.run(score_answer(document["questions"][0], malformed, document["commit"]))
    assert outcome["proseRefusal"] and not outcome["correct"]
    print("ok   reader_vs_root selftest (25 keyed answers, 25 neighbour citations, sign-test boundaries)")


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--selftest", action="store_true")
    parser.add_argument("--report", action="store_true")
    args = parser.parse_args(argv)
    if args.selftest:
        selftest()
        return 0
    if args.report:
        rows = [json.loads(line) for line in TRIALS.read_text().splitlines() if line.strip()]
        print(json.dumps(report(rows), indent=2, sort_keys=True))
        return 0
    parser.error("choose --selftest or --report")


if __name__ == "__main__":
    sys.exit(main())
