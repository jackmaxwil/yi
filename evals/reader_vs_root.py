#!/usr/bin/env python3
"""Paired root/reader children over 25 cited questions, with an exact paired kill verdict."""
from __future__ import annotations

import argparse
import ast
import asyncio
import json
import math
import os
import sys
import subprocess
import tempfile
from collections import defaultdict
from pathlib import Path
from unittest.mock import patch

ROOT = Path(__file__).resolve().parent.parent
FIXTURE = ROOT / "evals/fixtures/reader_vs_root/questions.json"
TRIALS = ROOT / "evals/trials/ns-b1-exp-d.jsonl"
sys.path.insert(0, str(ROOT / "python/yi_runtime/src"))
sys.path.insert(0, str(ROOT / "evals/adapters"))
import rlm  # noqa: E402
import run  # noqa: E402
import axes  # noqa: E402
import yi_usage  # noqa: E402
from yi.roles import verify_quotes  # noqa: E402

ALPHA = 0.025
ARMS = ("root", "reader")
REQUIRED_FAMILIES = ("kimi", "glm-flash")
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
            extra = {"name": f"{question['id']}-{arm}"}
            if arm == "reader":
                extra.update({"turns": 5, "schema": SCHEMA})
            code = (
                "import json\n"
                f"role = {arm!r}\n"
                f"child_prompt = {prompt!r}\n"
                f"partition = {partition!r}\n"
                f"extra = {extra!r}\n"
                "handle = None\n"
                "child = {}\n"
                "try:\n"
                "    handle = await rlm.run(child_prompt, role=role, partition=partition, **extra)\n"
                "    child = {'id': handle.rlm_child_id, 'name': handle.name,\n"
                "             'sessionDir': str(handle.session_dir), 'model': handle.model}\n"
                "    result = await handle.result(timeout=540)\n"
                "    payload = result.get('json')\n"
                "    if payload is None:\n"
                "        payload = json.loads(result['text'])\n"
                "except Exception as error:\n"
                "    payload = {'answer': '', 'quotes': [], 'proseRefusal': True,\n"
                "               'error': f'{type(error).__name__}: {error}'}\n"
                "finally:\n"
                "    if handle is not None:\n"
                "        await rlm.delete_subagent(handle)\n"
                "print(json.dumps({'child': child, 'payload': payload}, sort_keys=True))\n"
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


async def score_answer(question, answer):
    if not isinstance(answer, dict):
        return {"correct": False, "proseRefusal": True, "acceptedQuotes": []}
    kept = await verify_quotes(answer.get("quotes"), within=[question["url"]])
    accepted = [q for q in kept if q.get("url") == question["url"] and q.get("line") == question["line"]]
    correct = normalized(answer.get("answer", "")) == normalized(question["answer"]) and bool(accepted)
    return {"correct": correct, "proseRefusal": bool(answer.get("proseRefusal")), "acceptedQuotes": accepted}


def final_answer(row):
    path = Path(row.get("events") or "")
    if not path.is_absolute():
        path = axes.runs_root("EVAL_KILL_ROOT") / "b1-exp-d" / row.get("run", "") / "events.jsonl"
    try:
        return json.loads(run.final_answer(path, last_nonempty=False, separator="\n"))
    except (TypeError, ValueError):
        return None


def child_evidence(row, question, arm, envelope):
    def invalid(reason):
        return {"valid": False, "answer": None, "childTokens": None, "evidenceError": reason}

    if not isinstance(envelope, dict) or not isinstance(envelope.get("child"), dict):
        return invalid("outer answer has no child handle")
    handle = envelope["child"]
    if handle.get("name") != f"{question['id']}-{arm}" or handle.get("model") != row.get("model"):
        return invalid("child handle does not match the requested arm and model")
    child_id = handle.get("id")
    if not isinstance(child_id, str) or not child_id.startswith("sub-"):
        return invalid("child handle has no subagent id")
    base = axes.runs_root("EVAL_KILL_ROOT") / "b1-exp-d" / row.get("run", "") / "sessions"
    files = [p for p in base.rglob("*.jsonl") if any(part.startswith("sub-") for part in p.parts)] if base.is_dir() else []
    if len(files) != 1:
        return invalid(f"expected exactly one retained child session, found {len(files)}")
    path = files[0]
    if path.parent.name != child_id or Path(handle.get("sessionDir") or "") != path.parent:
        return invalid("child handle does not name the retained session")
    entries, malformed = yi_usage.json_lines(path)
    if malformed or not entries:
        return invalid("child session is unreadable")
    header = entries[0]
    if header.get("kind") != "header" or header.get("version") != 4 or not header.get("parentSessionId"):
        return invalid("child session has no valid parent header")
    roots = [p for p in base.rglob("*.jsonl") if not any(part.startswith("sub-") for part in p.parts)]
    root_entries, root_malformed = yi_usage.json_lines(roots[0]) if len(roots) == 1 else ([], 1)
    if root_malformed or not root_entries or root_entries[0].get("id") != header["parentSessionId"]:
        return invalid("child session does not belong to this outer run")
    prompt = child_prompt(question)
    prompted = False
    assistants = []
    for entry in entries:
        message = entry.get("message") if entry.get("kind") == "entry" else None
        if not isinstance(message, dict):
            continue
        content = message.get("content")
        text = content if isinstance(content, str) else "\n".join(
            block.get("text", "") for block in content or [] if isinstance(block, dict) and block.get("type") == "text"
        )
        prompted |= message.get("role") == "user" and prompt in text
        if message.get("role") == "assistant":
            assistants.append((message, text))
    if not prompted:
        return invalid("child session does not contain the requested question")
    if not assistants:
        return invalid("child session has no assistant answer")
    totals = {key: 0 for key in yi_usage.TOKEN_KEYS}
    for message, _ in assistants:
        usage = message.get("usage")
        if not isinstance(usage, dict) or usage.get("unknown") is True or any(
            not isinstance(usage.get(key), (int, float)) or isinstance(usage.get(key), bool)
            for key in yi_usage.TOKEN_KEYS
        ):
            return invalid("child assistant usage is missing or unknown")
        yi_usage._add_tokens(totals, usage)
    text = assistants[-1][1]
    try:
        answer = json.loads(text)
    except (TypeError, ValueError):
        answer = None
    payload = envelope.get("payload")
    if answer is None:
        if not isinstance(payload, dict) or not payload.get("proseRefusal"):
            return invalid("outer payload does not match the child answer")
        answer = {"answer": "", "quotes": [], "proseRefusal": True}
    elif payload != answer:
        return invalid("outer payload does not match the child answer")
    usage = {**totals, "assistantMessages": len(assistants), "sessionFiles": 1}
    return {"valid": True, "answer": answer, "childTokens": usage, "evidenceError": None}


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
    complete = [pair for pair in by_question.values() if set(pair) == set(ARMS) and all(row.get("valid", True) for row in pair.values())]
    improved = sum(pair["reader"]["correct"] and not pair["root"]["correct"] for pair in complete)
    regressed = sum(pair["root"]["correct"] and not pair["reader"]["correct"] for pair in complete)
    root_correct = sum(pair["root"]["correct"] for pair in complete)
    reader_correct = sum(pair["reader"]["correct"] for pair in complete)
    prose_refusals = sum(row["proseRefusal"] for row in scored if row["arm"] == "reader" and row.get("valid", True))
    p_value = sign_p_value(improved, regressed)
    return {
        "questions": len(complete),
        "rootCorrect": root_correct,
        "readerCorrect": reader_correct,
        "improved": improved,
        "regressed": regressed,
        "signP": p_value,
        "readerProseRefusals": prose_refusals,
        "kill": len(complete) == 25 and (p_value <= ALPHA or root_correct - reader_correct > 3 or prose_refusals > 0),
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
            envelope = final_answer(row)
            evidence = child_evidence(row, questions[qid], arm, envelope)
            answer = evidence.pop("answer")
            outcome = asyncio.run(score_answer(questions[qid], answer)) if evidence["valid"] else {
                "correct": False, "proseRefusal": False, "acceptedQuotes": [],
            }
            scored.append({
                "family": family,
                "question": qid,
                "arm": arm,
                "run": row.get("run"),
                "answer": answer,
                **outcome,
                **evidence,
            })
    families = {}
    for family in sorted({row["family"] for row in scored}):
        families[family] = family_verdict([row for row in scored if row["family"] == family])
    return {"commit": document["commit"], "families": families, "children": scored}


def summarize(rows):
    result = report(rows)
    complete = all(result["families"].get(family, {}).get("complete") for family in REQUIRED_FAMILIES)
    verdict = "KILL" if any(v["kill"] for v in result["families"].values()) else "KEEP" if complete else "UNRESOLVED"
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
            accepted = asyncio.run(score_answer(question, right))
            assert accepted["correct"], f"keyed answer rejected: {question['id']}"
            page = source(document["commit"], question["url"]).splitlines()
            neighbour_line = question["line"] + 1 if question["line"] < len(page) else question["line"] - 1
            neighbour = {"answer": question["answer"], "quotes": [{
                "url": question["url"], "line": neighbour_line, "text": page[neighbour_line - 1],
            }]}
            rejected = asyncio.run(score_answer(question, neighbour))
            assert not rejected["correct"], f"neighbour citation must not score: {question['id']}"
            wrong = {**right, "answer": "not the keyed answer"}
            assert not asyncio.run(score_answer(question, wrong))["correct"]
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
    outcome = asyncio.run(score_answer(document["questions"][0], malformed))
    assert outcome["proseRefusal"] and not outcome["correct"]
    def write_trial(root, children=1, usage=True, mismatch=False):
        question = document["questions"][0]
        answer = {"answer": question["answer"], "quotes": [{
            "url": question["url"], "line": question["line"], "text": question["quote"],
        }]}
        run = "kimi-q01-root-12345678"
        base = root / "b1-exp-d" / run
        child_dir = base / "sessions/root/children/sub-0"
        child_dir.mkdir(parents=True)
        envelope = {
            "child": {"id": "sub-0", "name": "q01-root", "sessionDir": str(child_dir), "model": "kimi/k3.1"},
            "payload": {"answer": "outer mismatch", "quotes": []} if mismatch else answer,
        }
        outer = {"type": "message_end", "message": {"role": "assistant", "content": [{
            "type": "text", "text": json.dumps(envelope),
        }]}}
        (base / "events.jsonl").write_text(json.dumps(outer) + "\n")
        header = {"kind": "header", "version": 4, "id": "root", "createdAt": 0, "cwd": "/work"}
        (base / "sessions/root.jsonl").write_text(json.dumps(header) + "\n")
        for index in range(children):
            directory = child_dir if index == 0 else child_dir.with_name(f"sub-{index}")
            directory.mkdir(exist_ok=True)
            child_header = {**header, "id": f"child-{index}", "parentSessionId": "root"}
            partition = (
                f'<<<yi-external 0123456789ab source="{question["url"]}" trust="untrusted">>>\n'
                "1:pinned source\n<<<end-yi-external 0123456789ab>>>\n\n"
            )
            user = {"kind": "entry", "type": "message", "message": {
                "role": "user", "content": f"[task from parent]\n\n{partition}{child_prompt(question)}",
            }}
            tokens = {"input": 12, "output": 3, "cacheRead": 0, "cacheWrite": 0} if usage else None
            assistant = {"kind": "entry", "type": "message", "message": {
                "role": "assistant", "content": [{"type": "text", "text": json.dumps(answer)}],
                **({"usage": tokens} if tokens else {}),
            }}
            (directory / "child.jsonl").write_text("".join(
                json.dumps(entry) + "\n" for entry in (child_header, user, assistant)
            ))
        return {"id": "q01-root", "family": "kimi", "run": run, "model": "kimi/k3.1"}

    def scored_trial(**kwargs):
        with tempfile.TemporaryDirectory() as scratch, patch.dict(os.environ, {"EVAL_KILL_ROOT": scratch}):
            row = write_trial(Path(scratch), **kwargs)
            return report([row])["children"][0]

    retained = scored_trial()
    assert retained["valid"] and retained["correct"], retained["evidenceError"]
    assert retained["childTokens"]["input"] == 12 and retained["childTokens"]["sessionFiles"] == 1
    with tempfile.TemporaryDirectory() as scratch:
        events = Path(scratch) / "events.jsonl"
        messages = [
            {"type": "message_end", "message": {"role": "assistant", "content": [{
                "type": "text", "text": json.dumps({"answer": "stale"}),
            }]}},
            {"type": "message_end", "message": {"role": "assistant", "content": []}},
        ]
        events.write_text("".join(json.dumps(message) + "\n" for message in messages))
        assert final_answer({"events": str(events)}) is None, "an empty final assistant must not reuse stale prose"
        messages[1]["message"]["content"] = [
            {"type": "text", "text": '{"answer":"ear'},
            {"type": "text", "text": 'lier"}'},
        ]
        events.write_text("".join(json.dumps(message) + "\n" for message in messages))
        assert final_answer({"events": str(events)}) is None, "separate text blocks must not concatenate into an envelope"
    for kwargs in ({"children": 0}, {"children": 2}, {"usage": False}, {"mismatch": True}):
        refused = scored_trial(**kwargs)
        assert not refused["valid"] and not refused["correct"] and refused["childTokens"] is None, kwargs
    prose = [dict(row, proseRefusal=row["arm"] == "reader" and row["question"] == "q01") for row in synthetic(25)]
    assert family_verdict(prose)["kill"], "one reader prose refusal crosses the kill boundary"
    clean = family_verdict([dict(row, correct=True) for row in synthetic(25)])
    one_family = {"families": {"kimi": clean}}
    with patch.object(sys.modules[__name__], "report", return_value=one_family):
        assert summarize([]).startswith("UNRESOLVED"), "one complete family is not enough for KEEP"
    both_families = {"families": {family: clean for family in REQUIRED_FAMILIES}}
    with patch.object(sys.modules[__name__], "report", return_value=both_families):
        assert summarize([]).startswith("KEEP"), "two complete clean families keep the route"
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
