#!/usr/bin/env python3
"""Judge replay: stage 0 of the seven-primitives plan, read-only (D258, #609).

    python3 evals/judge_replay.py all --dry --binary target/debug/yi --model faux/faux-1
    python3 evals/judge_replay.py extract --corpus yi:~/.yi/sessions \\
        --corpus claude:~/.claude/projects --out runs/replay
    python3 evals/judge_replay.py label --out runs/replay --model M --cap-usd 3
    python3 evals/judge_replay.py judge --out runs/replay --model M --prompt evals/replay/judge.md \\
        --split fit --limit 200 --cap-usd 3
    python3 evals/judge_replay.py match --out runs/replay --model M --cap-usd 1
    python3 evals/judge_replay.py report --out runs/replay

The bet under test: a judge that reads the owner's words by address, and nothing after the
boundary, predicts the owner's first objection. A boundary is a human message whose nearest
message ancestor is an assistant turn end; the judge sees the owner's earlier messages on that
tree path and the turn, and is scored against the message itself, labelled from what the owner
said and what the agent answered. Sessions split 70/30 into fit and held-out by the hash of the
session file name, never within a session.

Nothing is written under a corpus: every model call runs `yi ask` in an empty temporary cwd
under a temporary HOME, with its sessions under `--out`. A real run is the owner's, capped and
ledgered (evals/README.md); `--dry` is faux only and ships host-built replies.
"""

import argparse
import atexit
import concurrent.futures
import hashlib
import json
import os
import random
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT))
sys.path.insert(0, str(ROOT.parent / "skills" / "yi" / "session-mining"))

import extract  # noqa: E402
import run as runner  # noqa: E402
import yi_usage  # noqa: E402

REPLAY = ROOT / "replay"
FIXTURES = {"yi": ROOT / "fixtures" / "replay" / "yi", "claude": ROOT / "fixtures" / "replay" / "claude"}
LABELS = ("objected", "check_revealed", "accepted")
POSITIVE = ("objected", "check_revealed")
VERDICTS = ("accept", "revise", "escalate")
SCHEMAS = {"judge": "verdict", "label": "label", "match": "match"}
FIT_PERCENT = 70
INTENT_CHARS = 60_000
DIGEST_HEAD = 120
# macOS ARG_MAX is 1 MiB for argv and environment together and `yi ask` reads its prompt only
# from argv; half of it leaves the environment room.
ARGV_BYTES = 512_000
CALL_TIMEOUT_SEC = 600
BOOTSTRAP_DRAWS = 2000
SEED = 609

# Text a harness writes as a user message on the human's behalf: Yi's child notices, kernel
# restore and mailbox chase; Claude Code's commands, notices and interrupts, surveyed over 225
# top-level transcripts (evals/README.md, judge replay).
YI_INJECTED = ("[subagent ", "<ipython_state_restored>", "[host] request")
INJECTED = (
    "<command-name>", "<command-message>", "<local-command-stdout>", "<local-command-stderr>",
    "<local-command-caveat>", "<task-notification>", "<bash-input>", "<bash-stdout>",
    "<bash-stderr>", "<create-pr-command>", "Caveat:", "[Request interrupted",
    "This session is being continued",
)
REMINDERS = re.compile(r"(?:\s*<system-reminder>.*?</system-reminder>)+\s*", re.S)
DIGEST_KEYS = ("path", "file_path", "command", "pattern", "url", "code", "query", "description", "prompt")


def sha(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def split_of(corpus, name):
    """The session's side: its file name is the session id in both formats, so the split does
    not move with the corpus root, and one session never lands on both sides."""
    return "fit" if int(sha(f"{corpus}:{name}")[:8], 16) % 100 < FIT_PERCENT else "held-out"


def blocks_text(content):
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "\n".join(b.get("text") or "" for b in content if isinstance(b, dict) and b.get("type") == "text")


def digest(name, args):
    """One line per call: the tool and the head of what it touched."""
    args = args if isinstance(args, dict) else {}
    head = next((" ".join(str(args[key]).split()) for key in DIGEST_KEYS if args.get(key)), "")
    return {"name": name or "?", "head": head}


def read_yi(path):
    """Pi v4: a tree over `parentId`. A child session (a `parentSessionId` header, or a file in a
    `sub-*` directory, the layout a subagent writes) is skipped: its user is the parent agent."""
    header, entries, _corrupt = extract.read_session(path)
    if not header or header.get("parentSessionId") or any(p.startswith("sub-") for p in path.parent.parts):
        return {}
    nodes = {}
    for order, entry in enumerate(entries):
        if entry.get("kind") != "entry" or not entry.get("id"):
            continue
        message = entry.get("message") if entry.get("type") == "message" else None
        role = (message or {}).get("role")
        node = {"id": entry["id"], "parent": entry.get("parentId"), "order": order, "kind": "skip"}
        if role == "user" and blocks_text(message.get("content")).startswith(YI_INJECTED):
            node["kind"] = "prompt"
        elif role == "user":
            node.update(kind="human", text=blocks_text(message.get("content")), offset=0)
        elif role == "assistant":
            calls = [digest(b.get("name"), b.get("arguments")) for b in message.get("content") or []
                     if isinstance(b, dict) and b.get("type") == "toolCall"]
            node.update(kind="assistant", text=blocks_text(message.get("content")), calls=calls,
                        end=message.get("stopReason") != "toolUse", group=entry["id"])
        elif role == "toolResult":
            node["kind"] = "tool"
        nodes[entry["id"]] = node
    return nodes


def claude_human(entry):
    """The typed text of a Claude Code entry and where it starts in the stored text, or None.
    Typed means: not a sidechain, meta or compact-summary entry, not a tool result, an `origin`
    of `human` where the entry has one, and no harness prefix once leading reminders are cut."""
    if entry.get("isSidechain") or entry.get("isMeta") or entry.get("isCompactSummary"):
        return None
    if entry.get("type") == "attachment":
        attachment = entry.get("attachment") or {}
        if attachment.get("type") != "queued_command" or attachment.get("commandMode") != "prompt":
            return None
        origin, stored = attachment.get("origin"), blocks_text(attachment.get("prompt"))
    else:
        content = (entry.get("message") or {}).get("content")
        if isinstance(content, list) and any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content):
            return None
        origin, stored = entry.get("origin"), blocks_text(content)
    if isinstance(origin, dict) and origin.get("kind") != "human":
        return None
    cut = REMINDERS.match(stored)
    start = cut.end() if cut else 0
    text = stored[start:]
    if not text.strip() or text.startswith(INJECTED):
        return None
    return text, len(stored[:start].encode("utf-8"))


def read_claude(path):
    """Claude Code: a tree over `parentUuid`, bridged across a compaction by `logicalParentUuid`.
    A file under `subagents/` is a sidechain transcript and is skipped whole."""
    if "subagents" in path.parts:
        return {}
    nodes = {}
    for order, line in enumerate(path.read_text(errors="replace").splitlines()):
        try:
            entry = json.loads(line)
        except ValueError:
            continue
        if not isinstance(entry, dict) or not entry.get("uuid"):
            continue
        uuid = entry["uuid"]
        node = {"id": uuid, "parent": entry.get("parentUuid") or entry.get("logicalParentUuid"),
                "order": order, "kind": "skip"}
        message = entry.get("message") or {}
        human = claude_human(entry) if entry.get("type") in ("user", "attachment") else None
        if human:
            node.update(kind="human", text=human[0], offset=human[1])
        elif entry.get("type") == "assistant" and not entry.get("isSidechain"):
            calls = [digest(b.get("name"), b.get("input")) for b in message.get("content") or []
                     if isinstance(b, dict) and b.get("type") == "tool_use"]
            node.update(kind="assistant", text=blocks_text(message.get("content")), calls=calls,
                        end=message.get("stop_reason") != "tool_use", group=message.get("id") or uuid)
        elif entry.get("type") == "user":
            content = message.get("content")
            tool = isinstance(content, list) and any(isinstance(b, dict) and b.get("type") == "tool_result" for b in content)
            node["kind"] = "tool" if tool else "prompt"
        nodes[uuid] = node
    return nodes


READERS = {"yi": read_yi, "claude": read_claude}


def ancestors(nodes, node):
    seen = {node["id"]}
    parent = nodes.get(node.get("parent"))
    while parent and parent["id"] not in seen:
        seen.add(parent["id"])
        yield parent
        parent = nodes.get(parent.get("parent"))


def final_text(path_nodes, end):
    """The turn end's message text: Claude Code writes one entry per content block."""
    return "\n".join(n["text"] for n in path_nodes
                     if n["kind"] == "assistant" and n["group"] == end["group"] and n["text"]).strip()


def reply_to(nodes, children, human):
    """The agent's first reply: the first child at each step, down to the next turn end."""
    walked, node = [], human
    while True:
        kids = children.get(node["id"])
        if not kids:
            return ""
        node = kids[0]
        if node["kind"] == "human":
            return ""
        walked.append(node)
        if node["kind"] == "assistant" and node["end"]:
            # Every block entry of the last message carries its stop reason; take them all.
            while (kids := children.get(node["id"])) and kids[0]["kind"] == "assistant" and kids[0]["group"] == node["group"]:
                node = kids[0]
                walked.append(node)
            return final_text(walked, node)


def boundaries(corpus, path, nodes):
    children = {}
    for node in sorted(nodes.values(), key=lambda n: n["order"]):
        children.setdefault(node.get("parent"), []).append(node)
    rows = []
    for node in sorted(nodes.values(), key=lambda n: n["order"]):
        if node["kind"] != "human":
            continue
        chain = list(ancestors(nodes, node))
        line = [a for a in chain if a["kind"] not in ("skip", "prompt")]
        if not line or line[0]["kind"] != "assistant" or not line[0]["end"]:
            continue
        turn, end = [], line[0]
        # From the turn end back to what started the turn: a command or a notice can sit
        # between the end and the reply, as `/compact` does.
        for step in chain[chain.index(end):]:
            if step["kind"] in ("human", "prompt"):
                break
            turn.append(step)
        turn.reverse()
        address = lambda n: {"file": str(path), "entry": n["id"], "text": n["text"], "offset": n["offset"]}
        rows.append({
            "id": sha(f"{corpus}:{path.name}:{node['id']}")[:16],
            "corpus": corpus,
            "session": str(path),
            "split": split_of(corpus, path.name),
            "intent": [address(n) for n in reversed(chain) if n["kind"] == "human"],
            "turn": {"entry": end["id"], "text": final_text(turn, end),
                     "calls": [c for n in turn if n["kind"] == "assistant" for c in n["calls"]]},
            "next": address(node),
            "reply": reply_to(nodes, children, node),
        })
    return rows


def corpus_files(directory):
    return sorted(p for p in Path(directory).expanduser().rglob("*.jsonl") if not p.name.endswith(".telemetry.jsonl"))


def extract_corpora(specs):
    rows = []
    for kind, directory in specs:
        for path in corpus_files(directory):
            rows.extend(boundaries(kind, path, READERS[kind](path)))
    return rows


def resolve(text, quote):
    """The quote's byte range in the UTF-8 text, matched exactly after whitespace runs collapse
    to one space on both sides; None when the words are not there. Models cite by quote and the
    host computes bytes, because a byte index is not a character index."""
    wanted = " ".join(str(quote).split())
    if not wanted:
        return None
    kept, where = [], []
    for index, char in enumerate(text):
        if char.isspace():
            if kept and kept[-1] != " ":
                kept.append(" ")
                where.append(index)
        else:
            kept.append(char)
            where.append(index)
    at = "".join(kept).find(wanted)
    if at < 0:
        return None
    start, end = where[at], where[at + len(wanted) - 1] + 1
    return [len(text[:start].encode("utf-8")), len(text[:end].encode("utf-8"))]


def cite(boundary, citation):
    """A citation resolved against the message it names, as a byte range of that entry's text."""
    match = re.fullmatch(r"u(\d+)", str(citation.get("msg", "")))
    index = int(match.group(1)) if match else 0
    if not 1 <= index <= len(boundary["intent"]):
        return {**citation, "entry": None, "bytes": None}
    message = boundary["intent"][index - 1]
    found = resolve(message["text"], citation.get("quote", ""))
    span = [found[0] + message["offset"], found[1] + message["offset"]] if found else None
    return {**citation, "entry": message["entry"], "bytes": span}


def render_judge(boundary, cap=INTENT_CHARS):
    """The judge's whole input: the owner's messages as [u1]..[uN] and the turn, nothing after
    it. Under `cap` the oldest messages go first, and the cut says so where it is."""
    rows = [f"[u{i}] {m['text']}" for i, m in enumerate(boundary["intent"], 1)]
    kept, size = [], 0
    for row in reversed(rows):
        if kept and size + len(row) + 1 > cap:
            break
        kept.insert(0, row)
        size += len(row) + 1
    lines = ["# The owner's messages, oldest first"]
    if len(kept) < len(rows):
        lines.append(f"[… kept the newest {len(kept)} of {len(rows)} messages; intent_chars={cap} cut "
                     f"u1..u{len(rows) - len(kept)}; this input is all there is, the judge has no fetch]")
    lines += kept or ["(none)"]
    calls = boundary["turn"]["calls"]
    lines += ["", "# The turn the owner is reacting to", "## Tool calls, one per line"]
    lines += [f"- {c['name']}: {c['head'][:DIGEST_HEAD]}" if c["head"] else f"- {c['name']}" for c in calls] or ["(none)"]
    cut = sum(1 for c in calls if len(c["head"]) > DIGEST_HEAD)
    if cut:
        lines.append(f"[… {cut} of {len(calls)} call heads cut to digest_head={DIGEST_HEAD} chars; the judge has no fetch]")
    lines += ["", "## Final text", boundary["turn"]["text"] or "(no text)"]
    return "\n".join(lines)


def render_label(boundary):
    return "\n".join(["# The turn the agent ended", boundary["turn"]["text"] or "(no text)", "",
                      "# The owner's next message", boundary["next"]["text"], "",
                      "# The agent's reply to it", boundary["reply"] or "(no reply recorded)"])


def render_match(boundary, label, verdict):
    lines = ["# The owner's next message", boundary["next"]["text"], "",
             f"# The owner's objection: {label.get('objection', '')}",
             f"quote: {label.get('quote', '')}", "", "# The judge's predicted objections"]
    lines += [f"[{i}] {o.get('text', '')}" for i, o in enumerate(verdict.get("objections") or [])]
    return "\n".join(lines)


def faux_answer(phase, boundary):
    """--dry's host-built reply: valid JSON the pipeline can score, chosen by the id's hash."""
    bit = int(boundary["id"][0], 16) % 2 == 0
    if phase == "label":
        return {"label": "objected" if bit else "accepted", "objection": "faux objection" if bit else "",
                "quote": " ".join(boundary["next"]["text"].split()[:4]) if bit else ""}
    if phase == "match":
        return {"match": True, "index": 0}
    newest = boundary["intent"][-1:] if boundary["intent"] else []
    if not bit or not newest:
        return {"verdict": "accept", "objections": []}
    quote = " ".join(newest[0]["text"].split()[:6])
    return {"verdict": "revise", "objections": [
        {"text": "faux objection", "citations": [{"msg": f"u{len(boundary['intent'])}", "quote": quote}]}]}


_HOME, _HOME_LOCK = [], threading.Lock()


def home():
    """One temporary HOME per process: the caller's ~/.yi is never read or written, and the kernel
    is not prewarmed, since no judge runs a cell. Locked: --jobs threads ask for it at once."""
    with _HOME_LOCK:
        if not _HOME:
            _HOME.append(tempfile.mkdtemp(prefix="yi-replay-home-"))
            atexit.register(shutil.rmtree, _HOME[0], True)
            config = Path(_HOME[0]) / ".yi" / "config.json"
            config.parent.mkdir(parents=True)
            config.write_text(json.dumps({**yi_usage.eval_config(os.environ), "kernel": {"prewarm": False}}))
        return _HOME[0]


def parse_answer(text):
    try:
        return json.loads(text.strip())
    except ValueError:
        start = text.find("{")
        try:
            return json.JSONDecoder().raw_decode(text[start:])[0] if start >= 0 else None
        except ValueError:
            return None


def ask(args, phase, system, prompt, faux=None):
    """One `yi ask --json --confirm`: with no terminal an ask is a refusal, so only read-only
    tools can run, in an empty cwd under a temporary HOME; its session lands under --out."""
    if len(prompt.encode("utf-8")) + len(system.encode("utf-8")) > ARGV_BYTES:
        return {"error": f"input over argv_bytes={ARGV_BYTES}"}
    with tempfile.TemporaryDirectory(prefix="yi-replay-") as scratch:
        cwd, events = Path(scratch) / "cwd", Path(scratch) / "events.jsonl"
        cwd.mkdir()
        command = [args.binary, "ask", "--json", "--confirm", "--here", "--model", args.model,
                   "--system", system, "--schema", str(REPLAY / f"{SCHEMAS[phase]}.schema.json"),
                   "--session-dir", str(Path(args.out) / "sessions" / phase), "--cwd", str(cwd)]
        if faux is not None:
            script = Path(scratch) / "faux.jsonl"
            zero = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
            script.write_text(json.dumps({"role": "assistant", "content": [{"type": "text", "text": json.dumps(faux)}],
                                          "api": "faux", "provider": "faux", "model": "faux-1", "stopReason": "stop",
                                          "timestamp": 0, "usage": {**zero, "totalTokens": 0, "cost": {**zero, "total": 0}}}))
            command += ["--faux", str(script)]
        try:
            with events.open("w") as sink:
                done = subprocess.run([*command, prompt], stdout=sink, stderr=subprocess.PIPE, text=True,
                                      stdin=subprocess.DEVNULL, env={**os.environ, "HOME": home()},
                                      timeout=CALL_TIMEOUT_SEC)
        except subprocess.TimeoutExpired:
            return {"error": f"timed out after {CALL_TIMEOUT_SEC}s", "costUsd": yi_usage.parse_events(events)["costUsd"]}
        usage = {key: yi_usage.parse_events(events)[key] for key in ("costUsd", "input", "output", "cacheRead")}
        # Read-only tools still run under --confirm, and a read can reach past the boundary.
        usage["tools"] = sum(1 for event in yi_usage.json_lines(events)[0] if event.get("type") == "message_end"
                             for block in (event.get("message") or {}).get("content") or []
                             if isinstance(block, dict) and block.get("type") == "toolCall")
        answer = parse_answer(runner.final_answer(events))
    if done.returncode != 0 or not isinstance(answer, dict):
        return {"error": f"exit {done.returncode}: {done.stderr.strip()[-300:]}", **usage}
    return {"answer": answer, **usage}


def read_rows(path):
    rows, _malformed = yi_usage.json_lines(path)
    return rows


def by_id(rows, key="id"):
    return {row.get(key): row for row in rows}


def append(path, row):
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("a") as sink:
        sink.write(json.dumps(row, ensure_ascii=False) + "\n")


def effective_labels(out):
    """Labels, with the owner's marks winning: the latest mark per boundary."""
    labels = {k: v.get("label") for k, v in by_id(read_rows(out / "labels.jsonl")).items() if v.get("label")}
    for mark in sorted(read_rows(out / "marks.jsonl"), key=lambda m: m.get("ts", 0)):
        labels[mark.get("boundary")] = mark.get("label")
    return labels


def select(rows, split, limit, labels):
    """Deterministic by id (a hash). With labels, only labelled rows, up to limit/2 positive."""
    rows = sorted((r for r in rows if not split or r["split"] == split), key=lambda r: r["id"])
    if not labels:
        return rows[:limit] if limit else rows
    positives = [r for r in rows if labels.get(r["id"]) in POSITIVE]
    negatives = [r for r in rows if labels.get(r["id"]) == "accepted"]
    if not limit:
        return sorted(positives + negatives, key=lambda r: r["id"])
    taken = positives[: limit // 2]
    return sorted(taken + negatives[: limit - len(taken)], key=lambda r: r["id"])


def run_calls(args, phase, items, sink, build, keep):
    """Calls with --jobs in flight, each row appended as it lands; no call starts once --cap-usd
    is spent, but calls already in flight still land, so the overshoot is at most jobs - 1."""
    spent, stopped, pending, queue = 0.0, None, {}, list(items)
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        while queue or pending:
            while queue and len(pending) < args.jobs and not stopped:
                if args.cap_usd is not None and spent >= args.cap_usd:
                    stopped = f"cap ${args.cap_usd} reached at ${spent:.4f} with {len(queue)} {phase} calls left"
                    break
                system, prompt, faux, extra = build(queue.pop(0))
                pending[pool.submit(ask, args, phase, system, prompt, faux if args.dry else None)] = extra
            if not pending:
                break
            done, _ = concurrent.futures.wait(pending, return_when=concurrent.futures.FIRST_COMPLETED)
            for future in done:
                result = future.result()
                spent += result.get("costUsd") or 0.0
                # Exit 2 and 4 are refusals before any request (no key, unknown model): every
                # later call would repeat them.
                if result.get("error", "").startswith(("exit 2:", "exit 4:")):
                    stopped = stopped or f"yi refused: {result['error']}"
                append(sink, keep({**pending.pop(future), **result, "ts": int(time.time() * 1000)}))
    print(f"{phase}: spent ${spent:.4f}" + (f"; stopped: {stopped}" if stopped else ""), flush=True)
    return 1 if stopped and stopped.startswith("yi refused") else 0


def run_name(model, prompt_path):
    return f"{model.replace('/', '_')}-{sha(Path(prompt_path).read_text())[:12]}"


def cmd_extract(args):
    specs = [spec.split(":", 1) for spec in args.corpus]
    rows = extract_corpora(specs)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / ".gitignore").write_text("*\n")
    (out / "boundaries.jsonl").write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows))
    for corpus in sorted({r["corpus"] for r in rows}):
        mine = [r for r in rows if r["corpus"] == corpus]
        held = sum(1 for r in mine if r["split"] == "held-out")
        print(f"{corpus}: {len(mine)} boundaries in {len({r['session'] for r in mine})} sessions, {held} held-out")
    return 0


def cmd_label(args):
    out = Path(args.out)
    done = by_id(r for r in read_rows(out / "labels.jsonl") if r.get("label"))
    rows = [r for r in select(read_rows(out / "boundaries.jsonl"), args.split, args.limit, None) if r["id"] not in done]
    system = (REPLAY / "label.md").read_text()

    def build(row):
        return system, render_label(row), faux_answer("label", row), {"id": row["id"], "model": args.model}

    def keep(result):
        answer = result.pop("answer", None) or {}
        return {**result, **({"label": answer.get("label"), "objection": answer.get("objection", ""),
                              "quote": answer.get("quote", "")} if answer.get("label") in LABELS else {})}

    return run_calls(args, "label", rows, out / "labels.jsonl", build, keep)


def cmd_judge(args):
    out = Path(args.out)
    sink = out / "verdicts" / f"{run_name(args.model, args.prompt)}.jsonl"
    done = by_id(r for r in read_rows(sink) if r.get("verdict"))
    boundaries_by_id = by_id(read_rows(out / "boundaries.jsonl"))
    rows = [r for r in select(boundaries_by_id.values(), args.split, args.limit, effective_labels(out)) if r["id"] not in done]
    system, prompt_hash = Path(args.prompt).read_text(), sha(Path(args.prompt).read_text())

    def build(row):
        return system, render_judge(row), faux_answer("judge", row), {
            "id": row["id"], "split": row["split"], "corpus": row["corpus"], "model": args.model, "prompt": prompt_hash}

    def keep(result):
        answer = result.pop("answer", None) or {}
        if answer.get("verdict") not in VERDICTS:
            return result
        boundary = boundaries_by_id[result["id"]]
        objections = [{"text": o.get("text", ""), "citations": [cite(boundary, c) for c in o.get("citations") or []
                                                               if isinstance(c, dict)]}
                      for o in answer.get("objections") or [] if isinstance(o, dict)]
        return {**result, "verdict": answer["verdict"], "objections": objections}

    return run_calls(args, "judge", rows, sink, build, keep)


def cmd_match(args):
    out = Path(args.out)
    labels, boundaries_by_id = effective_labels(out), by_id(read_rows(out / "boundaries.jsonl"))
    label_rows = by_id(read_rows(out / "labels.jsonl"))
    system = (REPLAY / "match.md").read_text()
    for verdicts in sorted((out / "verdicts").glob("*.jsonl")):
        sink = out / "matches" / verdicts.name
        done = by_id(r for r in read_rows(sink) if "match" in r)
        rows = [v for v in by_id(read_rows(verdicts)).values()
                if v.get("verdict") in ("revise", "escalate") and labels.get(v["id"]) in POSITIVE and v["id"] not in done]

        def build(verdict):
            boundary = boundaries_by_id[verdict["id"]]
            label = label_rows.get(verdict["id"]) or {}
            return system, render_match(boundary, label, verdict), faux_answer("match", boundary), {
                "id": verdict["id"], "model": args.model}

        def keep(result):
            answer = result.pop("answer", None) or {}
            return {**result, **({"match": answer["match"], "index": answer.get("index", -1)}
                                 if isinstance(answer.get("match"), bool) else {})}

        if run_calls(args, "match", rows, sink, build, keep):
            return 1
    return 0


def balanced(pairs):
    """(balanced accuracy, recall, specificity) over (positive, flagged) pairs, or None when a
    class is empty and one of the two rates is undefined."""
    positives = [flagged for positive, flagged in pairs if positive]
    negatives = [flagged for positive, flagged in pairs if not positive]
    if not positives or not negatives:
        return None
    recall = sum(positives) / len(positives)
    specificity = sum(1 for flagged in negatives if not flagged) / len(negatives)
    return (recall + specificity) / 2, recall, specificity


def bootstrap(by_session, draws=BOOTSTRAP_DRAWS, seed=SEED):
    """95% interval of balanced accuracy, resampling whole sessions with replacement."""
    rng, sessions, scores = random.Random(seed), sorted(by_session), []
    for _ in range(draws):
        sample = [pair for _ in sessions for pair in by_session[rng.choice(sessions)]]
        scored = balanced(sample)
        if scored:
            scores.append(scored[0])
    if not scores:
        return None
    scores.sort()
    return scores[round(0.025 * (len(scores) - 1))], scores[round(0.975 * (len(scores) - 1))]


def resolution(verdicts):
    """Resolved quotes over all cited quotes; an objection that cites nothing is one unresolved."""
    total = resolved = 0
    for verdict in verdicts:
        for objection in verdict.get("objections") or []:
            cites = objection.get("citations") or [{"bytes": None}]
            total += len(cites)
            resolved += sum(1 for c in cites if c.get("bytes"))
    return (resolved / total if total else None), total


def section(verdicts, labels, matches, sessions):
    scored = [v for v in verdicts if v["id"] in labels]
    pairs = {}
    for v in scored:
        pairs.setdefault(sessions.get(v["id"], v["id"]), []).append((labels[v["id"]] in POSITIVE, v["verdict"] != "accept"))
    flat = [p for group in pairs.values() for p in group]
    scores, interval = balanced(flat), bootstrap(pairs)
    rate, cited = resolution(verdicts)
    positives = sum(1 for p, _ in flat if p)
    caught = sum(1 for v in scored if (matches.get(v["id"]) or {}).get("match") is True)
    return {"n": len(scored), "positive_rate": positives / len(flat) if flat else None,
            "resolution": rate, "cited": cited, "recall": scores[1] if scores else None,
            "specificity": scores[2] if scores else None, "balanced": scores[0] if scores else None,
            "interval": interval, "catch": caught / positives if positives else None,
            "cost": sum(v.get("costUsd") or 0.0 for v in verdicts)}


def fmt(value):
    return "  n/a" if value is None else f"{value:.3f}"


def gate_line(held):
    rate, low = held["resolution"], (held["interval"] or (None, None))[0]
    ok = rate is not None and rate >= 0.95 and low is not None and low > 0.5
    return (f"gate: {'PASS' if ok else 'FAIL'}: held-out citation resolution {fmt(rate)} (>= 0.95) and "
            f"balanced accuracy lower bound {fmt(low)} (> 0.5)")


def cmd_report(args):
    out = Path(args.out)
    labels = effective_labels(out)
    boundaries_by_id = by_id(read_rows(out / "boundaries.jsonl"))
    sessions = {k: v["session"] for k, v in boundaries_by_id.items()}
    label_cost = sum(r.get("costUsd") or 0.0 for r in read_rows(out / "labels.jsonl"))
    print(f"labels: {len(labels)} of {len(boundaries_by_id)} boundaries, ${label_cost:.4f}")
    for key in sorted({(b["corpus"], b["split"]) for b in boundaries_by_id.values()}):
        mine = [labels[k] for k, b in boundaries_by_id.items() if (b["corpus"], b["split"]) == key and k in labels]
        rate = sum(1 for label in mine if label in POSITIVE) / len(mine) if mine else None
        print(f"   {key[0]:8s}{key[1]:10s}{len(mine):5d} labelled, positive rate {fmt(rate)}")
    touched = set()
    for path in sorted((out / "verdicts").glob("*.jsonl")):
        rows = list(by_id(read_rows(path)).values())
        if any(row.get("split") == "held-out" for row in rows):
            touched.add(path.stem)
        verdicts = [r for r in rows if r.get("verdict") in VERDICTS]
        matches = by_id(read_rows(out / "matches" / path.name))
        match_cost = sum(r.get("costUsd") or 0.0 for r in matches.values())
        errors = [r.get("error") for r in rows if r.get("error") and r.get("verdict") not in VERDICTS]
        tooled = sum(1 for r in verdicts if r.get("tools"))
        print(f"\n== {path.stem}: {len(verdicts)} verdicts, {len(errors)} calls without one, {tooled} that ran a "
              f"read-only tool and may have read past the boundary; match ${match_cost:.4f}")
        for error in sorted(set(errors))[:5]:
            print(f"   error x{errors.count(error)}: {error}")
        print(f"   {'corpus':8s}{'split':10s}{'n':>5s}{'pos':>7s}{'cite':>7s}{'recall':>8s}{'spec':>7s}"
              f"{'bal':>7s}  95% interval   {'catch':>6s}{'cost':>9s}")
        held = None
        for split in ("fit", "held-out"):
            for corpus in sorted({v["corpus"] for v in verdicts}) + ["all"]:
                part = [v for v in verdicts if v["split"] == split and corpus in ("all", v["corpus"])]
                if not part:
                    continue
                s = section(part, labels, matches, sessions)
                if corpus == "all" and split == "held-out":
                    held = s
                low, high = s["interval"] or (None, None)
                print(f"   {corpus:8s}{split:10s}{s['n']:5d}{fmt(s['positive_rate']):>7s}{fmt(s['resolution']):>7s}"
                      f"{fmt(s['recall']):>8s}{fmt(s['specificity']):>7s}{fmt(s['balanced']):>7s}  "
                      f"[{fmt(low)}, {fmt(high)}]{fmt(s['catch']):>7s}{s['cost']:9.4f}")
        always_accept = balanced([(True, False), (False, False)])
        always_flag = balanced([(True, True), (False, True)])
        print(f"   baselines: always-accept {fmt(always_accept[0])}, always-flag {fmt(always_flag[0])}")
        print("   " + gate_line(held or {"resolution": None, "interval": None}))
    print("\njudge runs (model-prompt sha256) that have touched held-out: " + (", ".join(sorted(touched)) or "none"))
    return 0


def cmd_mark(args):
    if args.label not in LABELS:
        print(f"refused: label must be one of {', '.join(LABELS)}", file=sys.stderr)
        return 2
    append(Path(args.out) / "marks.jsonl", {"boundary": args.boundary, "label": args.label, "ts": int(time.time() * 1000)})
    return 0


def listing(directories):
    return sorted((str(p), p.stat().st_mtime_ns, p.stat().st_size)
                  for d in directories for p in Path(d).rglob("*") if p.is_file())


def cmd_all(args):
    """The dry pipeline end to end: every phase, then the proof that the corpus was only read."""
    if not args.dry:
        print("refused: `all` is the dry pipeline; a paid run is its phases, one at a time", file=sys.stderr)
        return 2
    specs = [spec.split(":", 1) for spec in args.corpus] or [[k, str(v)] for k, v in FIXTURES.items()]
    before = listing(d for _, d in specs)
    if not args.out:
        args.out = tempfile.mkdtemp(prefix="yi-replay-out-")
        atexit.register(shutil.rmtree, args.out, True)
    args.prompt, args.split, args.limit = str(REPLAY / "judge.md"), None, None
    args.corpus = [f"{k}:{d}" for k, d in specs]
    for phase in (cmd_extract, cmd_label, cmd_judge, cmd_match, cmd_report):
        if phase(args) != 0:
            return 1
    if listing(d for _, d in specs) != before:
        print("FAIL judge_replay_dry: a file under the corpus changed")
        return 1
    print("ok   judge_replay_dry (the corpus was only read)")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    commands = parser.add_subparsers(dest="command", required=True)
    for name in ("extract", "label", "judge", "match", "report", "mark", "all"):
        sub = commands.add_parser(name)
        sub.add_argument("--out", required=name != "all")
        if name in ("extract", "all"):
            sub.add_argument("--corpus", action="append", default=[], help="yi:<dir> or claude:<dir>; repeatable")
        if name in ("label", "judge", "match", "all"):
            sub.add_argument("--model", required=True)
            sub.add_argument("--binary", default=str(ROOT.parent / "target/debug/yi"))
            sub.add_argument("--jobs", type=int, default=1)
            sub.add_argument("--cap-usd", type=float, default=None)
            sub.add_argument("--dry", action="store_true", help="faux only, host-built replies, no spend")
        if name in ("label", "judge"):
            sub.add_argument("--split", choices=("fit", "held-out"))
            sub.add_argument("--limit", type=int, default=None)
        if name == "judge":
            sub.add_argument("--prompt", default=str(REPLAY / "judge.md"))
        if name == "mark":
            sub.add_argument("boundary")
            sub.add_argument("label")
    args = parser.parse_args(argv)
    if args.command == "extract" and not args.corpus:
        parser.error("extract needs --corpus yi:<dir> or claude:<dir>")
    if any(":" not in spec or spec.split(":", 1)[0] not in READERS for spec in getattr(args, "corpus", [])):
        parser.error("--corpus is yi:<dir> or claude:<dir>")
    if hasattr(args, "model"):
        # Preconditions by name (surface.py's rule): a gate spends no API budget.
        if args.dry != args.model.startswith("faux/"):
            print(f"refused: --dry runs faux only, and faux runs only under --dry, not {args.model}", file=sys.stderr)
            return 2
        if not args.dry and args.cap_usd is None:
            print("refused: --cap-usd is required for a real-model run (plan law 3)", file=sys.stderr)
            return 2
        if not Path(args.binary).is_file():
            print(f"refused: no binary at {args.binary} (cargo build -p yi-cli)", file=sys.stderr)
            return 2
    return globals()[f"cmd_{args.command}"](args)


if __name__ == "__main__":
    sys.exit(main())
