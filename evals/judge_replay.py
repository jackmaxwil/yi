#!/usr/bin/env python3
"""Judge replay: stage 0 of the seven-primitives plan, read-only (D259, #609).

    python3 evals/judge_replay.py all --dry --model faux/faux-1
    python3 evals/judge_replay.py extract --corpus yi:~/.yi/sessions \\
        --corpus claude:~/.claude/projects --out runs/replay
    python3 evals/judge_replay.py label --out runs/replay --model openrouter/M --cap-usd 3
    python3 evals/judge_replay.py judge --out runs/replay --model openrouter/M --arm self \\
        --split fit --limit 200 --cap-usd 3
    python3 evals/judge_replay.py report --out runs/replay

The bet under test: the working agent asked the owner's check (`--arm self`), or an outside
judge (`--arm judge`), reading the session up to a boundary and nothing after it, predicts the
owner's first objection. A boundary is a human message whose nearest message ancestor is an
assistant turn end; the input is the conversation on that tree path up to the turn end, the
owner's messages, the agent's text, its tool calls and their results, re-read from the source
file at call time. It is scored against the message itself, labelled from what the owner said
and what the agent answered. Conversations split 70/30 into fit and held-out by the hash of a
file name in each, never within one.

Nothing is written under a corpus. Every model call is one OpenRouter chat completion with no
tools: the phase's prompt and the rendered input are all the model receives, so it holds nothing
past the boundary and has no way to fetch it. A real run is the owner's, capped and ledgered
(evals/README.md); `--dry` is faux only, answers with host-built replies and makes no request.
"""

import argparse
import atexit
import bisect
import concurrent.futures
import functools
import hashlib
import http.client
import itertools
import json
import os
import random
import re
import shutil
import sys
import tempfile
import time
import urllib.error
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT.parent / "skills" / "yi" / "session-mining"))

import extract  # noqa: E402
import yi_usage  # noqa: E402

REPLAY = ROOT / "replay"
FIXTURES = {"yi": ROOT / "fixtures" / "replay" / "yi", "claude": ROOT / "fixtures" / "replay" / "claude"}
LABELS = ("objected", "check_revealed", "accepted")
KINDS = ("intent_loss", "new_info")
# A boundary's class: v2 labels split `objected` by what it rests on, v1's labels.jsonl keeps it
# whole. `new_info` is unpredictable by construction, so it is counted and never scored.
CLASSES = ("intent_loss", "new_info", "check_revealed", "accepted")
POSITIVE = ("objected", "intent_loss", "check_revealed")
VERDICTS = ("accept", "revise", "escalate")
ARMS = ("self", "judge")
SCHEMAS = {"judge": "verdict", "label": "label"}
FIT_PERCENT = 70
INTENT_CHARS = 60_000
# The whole prefix, about 60k tokens at four chars a token: the smallest target context is Opus
# 5.5's 1M, and the owner's messages alone peaked at 216,798 chars on the 2026-09-26 corpus.
PREFIX_CHARS = 240_000
TOOL_CHARS = 2_000
OWNER, AGENT, RESULT = "owner", "agent", "result"
OPENROUTER = "https://openrouter.ai/api/v1/chat/completions"
KEY = "OPENROUTER_API_KEY"
CALL_TIMEOUT_SEC = 300
RETRIES = 3
# Statuses every later call would repeat: a bad key, no credit, a refused or unknown model.
REFUSALS = (401, 402, 403, 404)
# Yi's own rule (D25, `Attribution::reads_as_typed`): from this instant a typed message carries
# `attribution: user` and any other user-role message is the host's.
ATTRIBUTED_SINCE_MS = 1_788_256_519_000
BOOTSTRAP_DRAWS = 2000
SEED = 609

# Text a harness writes as a user message on the human's behalf: Yi's child notices, kernel
# restore and mailbox chase before attribution; Claude Code's commands, notices and interrupts,
# surveyed over 225 top-level transcripts (evals/README.md, judge replay).
YI_INJECTED = ("[subagent ", "<ipython_state_restored>", "[host] request")
INJECTED = (
    "<command-name>", "<command-message>", "<local-command-stdout>", "<local-command-stderr>",
    "<local-command-caveat>", "<task-notification>", "<bash-input>", "<bash-stdout>",
    "<bash-stderr>", "<create-pr-command>", "Caveat:", "[Request interrupted",
    "This session is being continued",
)
REMINDERS = re.compile(r"(?:\s*<system-reminder>.*?</system-reminder>)+\s*", re.S)


def sha(text):
    return hashlib.sha256(text.encode("utf-8")).hexdigest()


def split_of(corpus, name):
    """A conversation's side, by the first file name in it: the name is the session id in both
    formats, so the split does not move with the corpus root."""
    return "fit" if int(sha(f"{corpus}:{name}")[:8], 16) % 100 < FIT_PERCENT else "held-out"


def blocks_text(content):
    if isinstance(content, str):
        return content
    if not isinstance(content, list):
        return ""
    return "\n".join(b.get("text") or "" for b in content if isinstance(b, dict) and b.get("type") == "text")


def call(block, args):
    return {"id": block.get("id"), "name": block.get("name") or "?", "args": block.get(args)}


def read_yi(path):
    """Pi v4: a tree over `parentId`. A child session (a `parentSessionId` header, or a file in a
    `sub-*` directory, the layout a subagent writes) is skipped: its user is the parent agent.
    The host's user-role notices (plan nudges, child notices) are prompts, not the owner."""
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
        if role == "user":
            text = blocks_text(message.get("content"))
            if (entry.get("timestamp") or 0) >= ATTRIBUTED_SINCE_MS:
                typed = message.get("attribution") == "user"
            else:
                typed = not text.startswith(YI_INJECTED)
            node.update({"kind": "human", "text": text, "offset": 0} if typed else {"kind": "prompt"})
        elif role == "assistant":
            calls = [call(b, "arguments") for b in message.get("content") or []
                     if isinstance(b, dict) and b.get("type") == "toolCall"]
            node.update(kind="assistant", text=blocks_text(message.get("content")), calls=calls,
                        end=message.get("stopReason") != "toolUse", group=entry["id"])
        elif role == "toolResult":
            node.update(kind="tool", results=[{"call": message.get("toolCallId"),
                                               "text": blocks_text(message.get("content"))}])
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
    # Incident: `splitlines` also splits at U+2028, which 13 typed messages carried raw.
    for order, line in enumerate(path.read_text(errors="replace").split("\n")):
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
            calls = [call(b, "input") for b in message.get("content") or []
                     if isinstance(b, dict) and b.get("type") == "tool_use"]
            node.update(kind="assistant", text=blocks_text(message.get("content")), calls=calls,
                        end=message.get("stop_reason") != "tool_use", group=message.get("id") or uuid)
        elif entry.get("type") == "user":
            content = message.get("content")
            results = [{"call": b.get("tool_use_id"), "text": blocks_text(b.get("content"))}
                       for b in content if isinstance(b, dict) and b.get("type") == "tool_result"] if isinstance(content, list) else []
            node.update(kind="tool" if results else "prompt", results=results)
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
        # Back from the turn end to what opened the turn: the owner, or a command, notice or summary
        # right after a turn end. A notice mid-turn (a skill body, a plan nudge) opened nothing.
        for at in range(chain.index(end), len(chain)):
            step = chain[at]
            if step["kind"] == "human":
                break
            if step["kind"] == "prompt":
                before = next((a for a in chain[at + 1:] if a["kind"] not in ("skip", "prompt")), None)
                if before is None or before["kind"] == "human" or (before["kind"] == "assistant" and before["end"]):
                    break
            turn.append(step)
        turn.reverse()
        address = lambda n: {"file": str(path), "entry": n["id"], "text": n["text"], "offset": n["offset"]}
        rows.append({
            "id": sha(f"{corpus}:{node['id']}")[:16],
            "corpus": corpus,
            "session": str(path),
            "intent": [address(n) for n in reversed(chain) if n["kind"] == "human"],
            "turn": {"entry": end["id"], "text": final_text(turn, end)},
            "next": address(node),
            "reply": reply_to(nodes, children, node),
        })
    return rows


def corpus_files(directory):
    return sorted(p for p in Path(directory).expanduser().rglob("*.jsonl") if not p.name.endswith(".telemetry.jsonl"))


def extract_corpora(specs):
    """Each boundary once, split by conversation. Claude Code rewrites a resumed transcript into
    a new file under the same entry ids, so files that share or bridge to an entry are one
    conversation, and a boundary found in several keeps its fullest intent record."""
    rows, owner, root = {}, {}, {}

    def find(session):
        while root.get(session, session) != session:
            session = root[session]
        return session

    for kind, directory in specs:
        for path in corpus_files(directory):
            nodes = READERS[kind](path)
            for key in {*nodes, *(n["parent"] for n in nodes.values() if n.get("parent"))}:
                one, two = find(owner.setdefault((kind, key), (kind, path.name))), find((kind, path.name))
                if one != two:
                    root[max(one, two)] = min(one, two)
            for row in boundaries(kind, path, nodes):
                kept = rows.get(row["id"])
                if kept is None or len(row["intent"]) > len(kept["intent"]):
                    rows[row["id"]] = row
    # ponytail: a conversation's side follows its first file name, so re-extracting a grown corpus
    # into the same --out can move one whose new resumed file sorts first; extract once per run.
    for row in rows.values():
        kind, name = find((row["corpus"], Path(row["session"]).name))
        row.update(conversation=f"{kind}:{name}", split=split_of(kind, name))
    return list(rows.values())


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


def newest(rows, cap):
    """How many of the newest rows fit in `cap` chars, a newline each; never fewer than one."""
    kept = size = 0
    for row in reversed(rows):
        if kept and size + len(row) + 1 > cap:
            break
        kept, size = kept + 1, size + len(row) + 1
    return kept


def owner_cut(kept, total, cap):
    return f"[… kept the newest {kept} of {total} messages; intent_chars={cap} cut u1..u{total - kept}]"


def owner_rows(intent, cap=INTENT_CHARS):
    """The owner's messages as [u1]..[uN]; under `cap` the oldest go first, and the cut says so."""
    rows = [f"[u{i}] {m['text']}" for i, m in enumerate(intent, 1)]
    kept = newest(rows, cap)
    return ([owner_cut(kept, len(rows), cap)] if kept < len(rows) else []) + rows[len(rows) - kept:] or ["(none)"]


def clip(text, cap, what):
    """`text` whole, or its head under `cap` chars and the row that says so where it is cut."""
    if len(text) <= cap:
        return [text]
    return [text[:cap], f"[… kept {cap} of {len(text)} chars{what}; tool_chars={cap}]"]


def call_lines(item, cap):
    """A call's name, then each argument on its own lines, every value clipped alone: a file's
    path survives the cut of its content."""
    args = item["args"] if isinstance(item["args"], dict) else {"arguments": item["args"]} if item["args"] else {}
    lines = [f"[call {item['name']}]"]
    for key, value in args.items():
        head, *cut = clip(value if isinstance(value, str) else json.dumps(value, ensure_ascii=False), cap, f" of `{key}`")
        lines += [f"{key}: {head}", *cut]
    return lines


def conversation(row, nodes, tool_chars):
    """What the owner saw before the boundary, as (tier, lines) in file order: the path's owner
    messages and agent text, every other block of an assistant message on it, and each result of
    a call shown. Claude Code hangs parallel calls and their results off the path, so the path
    alone loses them. Nothing at or after the boundary is read, nor anything on another branch."""
    boundary = nodes.get(row["next"]["entry"])
    if boundary is None:
        raise LookupError(f"the boundary entry {row['next']['entry']} is not in {row['session']}; extract again")
    path = list(ancestors(nodes, boundary))
    owners = [n["id"] for n in reversed(path) if n["kind"] == "human"]
    if owners != [m["entry"] for m in row["intent"]]:
        raise LookupError(f"{row['session']} changed since extract: the owner's messages on this path differ; extract again")
    before = sorted((n for n in nodes.values() if n["order"] < boundary["order"]), key=lambda n: n["order"])
    groups = {n["group"] for n in path if n["kind"] == "assistant"}
    shown = {n["id"] for n in path} | {n["id"] for n in before if n["kind"] == "assistant" and n["group"] in groups}
    names = {c["id"]: c["name"] for n in before if n["id"] in shown and n["kind"] == "assistant" for c in n["calls"]}
    number, items = {entry: i for i, entry in enumerate(owners, 1)}, []
    for node in before:
        if node["kind"] == "human" and node["id"] in number:
            items.append((OWNER, [f"[u{number[node['id']]}] {node['text']}"]))
        elif node["kind"] == "assistant" and node["id"] in shown:
            items += [(AGENT, [f"[agent] {node['text']}"])] if node["text"] else []
            items += [(AGENT, call_lines(c, tool_chars)) for c in node["calls"]]
        elif node["kind"] == "tool":
            items += [(RESULT, [f"[result {names.get(r['call'], '?')}]", *clip(r["text"], tool_chars, "")])
                      for r in node["results"] if r["call"] in names or node["id"] in shown]
    return items


def fit(items, cap):
    """The items joined in at most `cap` chars: whole tool results go oldest first, then agent
    text and calls oldest first, never an owner's message. One row on top names kept of total and
    the cap; a short row marks each gap. A cut that costs more than it saves waits for a neighbour."""
    texts = ["\n".join(lines) for _, lines in items]
    whole = "\n".join(texts)
    if len(whole) <= cap:
        return whole
    order = [i for tier in (RESULT, AGENT) for i, (kind, _) in enumerate(items) if kind == tier]
    total = {tier: sum(1 for kind, _ in items if kind == tier) for tier in (RESULT, AGENT)}
    gap = lambda n: f"[… {n} cut: prefix_chars]"
    summary = lambda kept: (f"[… prefix_chars={cap} kept {kept[RESULT]} of {total[RESULT]} tool results, "
                            f"{kept[AGENT]} of {total[AGENT]} agent texts and calls; each \"{gap('n')}\" row is a gap]")
    # Rows priced at their widest, so `size` bounds the rendered length from above.
    row, cut = len(gap(len(items))) + 1, set()
    size = len(whole) + len(summary(total)) + 1

    def saving(i):
        joined = (i - 1 in cut) + (i + 1 in cut)
        return len(texts[i]) + 1 + (row if joined == 2 else 0 if joined else -row)

    for paying in (True, False):
        for i in order:
            if size <= cap:
                break
            if i not in cut and (saving(i) > 0 or not paying):
                size -= saving(i)
                cut.add(i)
    kept = {tier: total[tier] - sum(1 for i in cut if items[i][0] == tier) for tier in total}
    lines, run = [summary(kept)], 0
    for i, text in enumerate(texts):
        if i not in cut:
            lines.append(text)
            continue
        run += 1
        if i + 1 not in cut:
            lines.append(gap(run))
            run = 0
    return "\n".join(lines)


def prefix(row, nodes, tool_chars=TOOL_CHARS, prefix_chars=PREFIX_CHARS, intent_chars=INTENT_CHARS):
    """The input both arms read. When the owner's messages alone pass `prefix_chars`, the oldest
    go by the intent_chars rule first, then `fit` cuts the rest."""
    items = conversation(row, nodes, tool_chars)
    owners = [i for i, (kind, _) in enumerate(items) if kind == OWNER]
    rows = [items[i][1][0] for i in owners]
    kept = newest(rows, intent_chars)
    if len("\n".join(rows)) > prefix_chars and kept < len(rows):
        gone = set(owners[1:len(rows) - kept])
        items = [(OWNER, [owner_cut(kept, len(rows), intent_chars)]) if i == owners[0] else item
                 for i, item in enumerate(items) if i not in gone]
    return fit(items, prefix_chars)


def render_label(boundary):
    return "\n".join(["# The owner's earlier messages, oldest first", *owner_rows(boundary["intent"]), "",
                      "# The turn the agent ended", boundary["turn"]["text"] or "(no text)", "",
                      "# The owner's next message", boundary["next"]["text"], "",
                      "# The agent's reply to it", boundary["reply"] or "(no reply recorded)"])


def faux_answer(phase, boundary):
    """--dry's host-built reply: valid JSON the pipeline can score, chosen by the id's hash. A
    faux intent_loss on a boundary with no earlier owner message cites nothing and is demoted."""
    bit = int(boundary["id"][0], 16) % 2 == 0
    newest_quote = [{"msg": f"u{len(boundary['intent'])}", "quote": " ".join(boundary["intent"][-1]["text"].split()[:6])}
                    ] if boundary["intent"] else []
    if phase == "label":
        return {"label": "objected" if bit else "accepted", "kind": "intent_loss" if bit else "",
                "objection": "faux objection" if bit else "",
                "quote": " ".join(boundary["next"]["text"].split()[:4]) if bit else "",
                "rests_on": newest_quote if bit else []}
    if not bit or not newest_quote:
        return {"verdict": "accept", "objections": [], "p_objection": 0.2}
    return {"verdict": "revise", "objections": [{"text": "faux objection", "citations": newest_quote}], "p_objection": 0.8}


def parse_answer(text):
    """The reply as JSON, else the first `{...}` in it: models fence JSON. None when neither parses."""
    try:
        return json.loads(text.strip())
    except ValueError:
        start = text.find("{")
        try:
            return json.JSONDecoder().raw_decode(text[start:])[0] if start >= 0 else None
        except ValueError:
            return None


def request(model, phase, system, turns, effort=None):
    """The whole call: the phase's prompt, the rendered input and the self arm's check as user
    turns, and no tools, so blindness holds by construction. Cost comes back as `usage.cost`."""
    schema = json.loads((REPLAY / f"{SCHEMAS[phase]}.schema.json").read_text())
    body = {"model": model.removeprefix("openrouter/"), "temperature": 0,
            "messages": [{"role": "system", "content": system}, *({"role": "user", "content": t} for t in turns)],
            "response_format": {"type": "json_schema",
                                "json_schema": {"name": SCHEMAS[phase], "strict": True, "schema": schema}},
            "usage": {"include": True}}
    if effort:
        body["reasoning"] = {"effort": effort}
    return body


def post(body):
    """One chat completion, retried with backoff on 429 and 5xx at most RETRIES times. The key
    rides a header only, never argv or a row."""
    data = json.dumps(body).encode("utf-8")
    headers = {"Authorization": f"Bearer {os.environ.get(KEY, '')}", "Content-Type": "application/json"}
    for attempt in range(RETRIES + 1):
        try:
            with urllib.request.urlopen(urllib.request.Request(OPENROUTER, data, headers),
                                        timeout=CALL_TIMEOUT_SEC) as response:
                return json.loads(response.read())
        except urllib.error.HTTPError as error:
            if attempt == RETRIES or (error.code != 429 and error.code < 500):
                raise
            error.close()
        time.sleep(2 ** attempt)


def ask(args, phase, system, turns, faux=None):
    """One call, as a row: the answer or an error, with the provider's model id, tokens, cost and
    latency. `faux` is --dry's host-built answer, handed back in the provider's shape at no cost."""
    body, started = request(args.model, phase, system, turns, args.effort), time.monotonic()
    try:
        reply = post(body) if faux is None else {
            "model": args.model, "choices": [{"message": {"content": json.dumps(faux)}}],
            "usage": {"prompt_tokens": 0, "completion_tokens": 0, "cost": 0}}
    except urllib.error.HTTPError as error:
        with error:
            detail = error.read().decode("utf-8", "replace")[:300]
        return {"error": f"{'refused: ' if error.code in REFUSALS else ''}http {error.code}: {detail}"}
    except (OSError, ValueError, http.client.HTTPException) as error:
        return {"error": f"{type(error).__name__}: {error}"}
    usage = reply.get("usage") or {}
    row = {"providerModel": reply.get("model"), "input": usage.get("prompt_tokens"),
           "output": usage.get("completion_tokens"),
           "cached": (usage.get("prompt_tokens_details") or {}).get("cached_tokens"),
           "costUsd": usage.get("cost"), "latencyMs": round((time.monotonic() - started) * 1000)}
    if reply.get("error"):
        return {"error": f"provider: {reply['error']}", **row}
    if not isinstance(row["costUsd"], (int, float)):
        return {"error": "refused: the reply carried no usage.cost, so --cap-usd cannot count it", **row}
    content = ((reply.get("choices") or [{}])[0].get("message") or {}).get("content") or ""
    answer = parse_answer(content)
    if not isinstance(answer, dict):
        return {"error": "no JSON object in the reply", "content": content, **row}
    return {"answer": answer, **row}


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
    """Each boundary's class, the owner messages its label rests on, and the file they came from:
    labels-v2.jsonl when it has rows, else v1's labels.jsonl. The latest owner mark wins."""
    v2 = [r for r in read_rows(out / "labels-v2.jsonl") if r.get("label") in LABELS]
    rows = v2 or [r for r in read_rows(out / "labels.jsonl") if r.get("label") in LABELS]
    classes = {r["id"]: (r.get("kind") or "new_info") if v2 and r["label"] == "objected" else r["label"] for r in rows}
    rests = {r["id"]: {c.get("msg") for c in r.get("rests_on") or [] if c.get("bytes")} for r in v2}
    for mark in sorted(read_rows(out / "marks.jsonl"), key=lambda m: m.get("ts", 0)):
        classes[mark.get("boundary")] = mark.get("label") if v2 or mark.get("label") not in KINDS else "objected"
    return classes, rests, "labels-v2.jsonl" if v2 else "labels.jsonl"


def select(rows, split, limit, classes):
    """Deterministic by id (a hash). With classes: scored rows only, positives and negatives
    interleaved p, n, p, n in id order, so a cut by --limit or --cap-usd stays balanced."""
    rows = sorted((r for r in rows if not split or r["split"] == split), key=lambda r: r["id"])
    if classes:
        pairs = itertools.zip_longest([r for r in rows if classes.get(r["id"]) in POSITIVE],
                                      [r for r in rows if classes.get(r["id"]) == "accepted"])
        rows = [r for pair in pairs for r in pair if r is not None]
    return rows[:limit] if limit else rows


def run_calls(args, phase, items, sink, build, keep):
    """Calls with --jobs in flight, each row appended as it lands; no call starts once --cap-usd
    is spent, but calls already in flight still land, so the overshoot is at most jobs - 1. A
    build that returns no system prompt lands its error as the row, unasked."""
    spent, stopped, pending, queue = 0.0, None, {}, list(items)
    with concurrent.futures.ThreadPoolExecutor(max_workers=args.jobs) as pool:
        while queue or pending:
            while queue and len(pending) < args.jobs and not stopped:
                if args.cap_usd is not None and spent >= args.cap_usd:
                    stopped = f"cap ${args.cap_usd} reached at ${spent:.4f} with {len(queue)} {phase} calls left"
                    break
                system, turns, faux, extra = build(queue.pop(0))
                if system is None:
                    append(sink, keep({**extra, "error": turns, "ts": int(time.time() * 1000)}))
                    continue
                pending[pool.submit(ask, args, phase, system, turns, faux if args.dry else None)] = extra
            if not pending:
                break
            done, _ = concurrent.futures.wait(pending, return_when=concurrent.futures.FIRST_COMPLETED)
            for future in done:
                result = future.result()
                spent += result.get("costUsd") or 0.0
                if result.get("error", "").startswith("refused:"):
                    stopped = stopped or result["error"]
                append(sink, keep({**pending.pop(future), **result, "ts": int(time.time() * 1000)}))
    print(f"{phase}: spent ${spent:.4f}" + (f"; stopped: {stopped}" if stopped else ""), flush=True)
    return 1 if stopped and stopped.startswith("refused:") else 0


def run_name(model, arm, frame, effort=None):
    """The run key: model, arm, the hash of everything the arm adds to the prefix, and effort."""
    return f"{model.replace('/', '_')}-{arm}-{sha(frame)[:12]}" + (f"-{effort}" if effort else "")


def framing(arm, prompt=None):
    """(system prompt, turns after the prefix): the arm's prompt and the shared answer spec; the
    self arm then asks the owner's check as the final user turn."""
    system = Path(prompt or REPLAY / f"{arm}.md").read_text() + "\n" + (REPLAY / "answer.md").read_text()
    return system, [(REPLAY / "check.md").read_text().strip()] if arm == "self" else []


def cmd_extract(args):
    specs = [spec.split(":", 1) for spec in args.corpus]
    rows = extract_corpora(specs)
    out = Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    (out / ".gitignore").write_text("*\n")
    (out / "boundaries.jsonl").write_text("".join(json.dumps(r, ensure_ascii=False) + "\n" for r in rows))
    (out / "corpora.json").write_text(json.dumps([[kind, str(Path(d).expanduser().resolve())] for kind, d in specs]))
    for corpus in sorted({r["corpus"] for r in rows}):
        mine = [r for r in rows if r["corpus"] == corpus]
        held = sum(1 for r in mine if r["split"] == "held-out")
        print(f"{corpus}: {len(mine)} boundaries in {len({r['session'] for r in mine})} sessions, "
              f"{len({r['conversation'] for r in mine})} conversations, {held} held-out")
    return 0


def cmd_label(args):
    """labels-v2.jsonl; v1's labels.jsonl is never written again. An `intent_loss` whose
    `rests_on` resolves to no owner message is demoted to `new_info`."""
    out = Path(args.out)
    sink = out / "labels-v2.jsonl"
    done = by_id(r for r in read_rows(sink) if r.get("label"))
    boundaries_by_id = by_id(read_rows(out / "boundaries.jsonl"))
    rows = [r for r in select(boundaries_by_id.values(), args.split, args.limit, None) if r["id"] not in done]
    system = (REPLAY / "label.md").read_text()

    def build(row):
        return system, [render_label(row)], faux_answer("label", row), {"id": row["id"], "model": args.model,
                                                                        "effort": args.effort}

    def keep(result):
        answer = result.pop("answer", None) or {}
        if answer.get("label") not in LABELS:
            return result
        kind = (answer.get("kind") if answer.get("kind") in KINDS else "new_info") if answer["label"] == "objected" else ""
        rests = [cite(boundaries_by_id[result["id"]], c) for c in answer.get("rests_on") or [] if isinstance(c, dict)]
        demoted = kind == "intent_loss" and not any(c["bytes"] for c in rests)
        return {**result, "label": answer["label"], "kind": "new_info" if demoted else kind, "demoted": demoted,
                "objection": answer.get("objection", ""), "quote": answer.get("quote", ""), "rests_on": rests}

    return run_calls(args, "label", rows, sink, build, keep)


def cmd_judge(args):
    out = Path(args.out)
    system, turns = framing(args.arm, args.prompt)
    frame = "\n".join([system, *turns])
    sink = out / "verdicts" / f"{run_name(args.model, args.arm, frame, args.effort)}.jsonl"
    done = by_id(r for r in read_rows(sink) if r.get("verdict"))
    boundaries_by_id = by_id(read_rows(out / "boundaries.jsonl"))
    rows = [r for r in select(boundaries_by_id.values(), args.split, args.limit, effective_labels(out)[0])
            if r["id"] not in done]
    # ponytail: rows arrive interleaved by hash, so this cache rarely hits; group by file if reads dominate.
    nodes = functools.lru_cache(maxsize=4)(lambda corpus, session: READERS[corpus](Path(session)))

    def build(row):
        extra = {"id": row["id"], "split": row["split"], "corpus": row["corpus"], "model": args.model,
                 "arm": args.arm, "prompt": sha(frame), "effort": args.effort}
        try:
            shown = prefix(row, nodes(row["corpus"], row["session"]))
        except (LookupError, OSError) as error:
            return None, f"{type(error).__name__}: {error}", None, extra
        return system, [shown, *turns], faux_answer("judge", row), extra

    def keep(result):
        answer = result.pop("answer", None)
        if answer is None:
            return result
        chance = answer.get("p_objection")
        if answer.get("verdict") not in VERDICTS or isinstance(chance, bool) or not isinstance(chance, (int, float)) \
                or not 0 <= chance <= 1:
            return {**result, "error": "no verdict with a p_objection in [0, 1]", "content": json.dumps(answer)}
        boundary = boundaries_by_id[result["id"]]
        objections = [{"text": o.get("text", ""), "citations": [cite(boundary, c) for c in o.get("citations") or []
                                                               if isinstance(c, dict)]}
                      for o in answer.get("objections") or [] if isinstance(o, dict)]
        return {**result, "verdict": answer["verdict"], "objections": objections, "p_objection": chance}

    return run_calls(args, "judge", rows, sink, build, keep)


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


def auroc(pairs):
    """Mann-Whitney AUROC over (positive, score) pairs by mean rank, ties counting half: the
    chance a positive outscores a negative. None when a class is empty."""
    positives = [score for positive, score in pairs if positive]
    negatives = len(pairs) - len(positives)
    if not positives or not negatives:
        return None
    ordered = sorted(score for _, score in pairs)
    ranks = sum((bisect.bisect_left(ordered, s) + bisect.bisect_right(ordered, s) + 1) / 2 for s in positives)
    return (ranks - len(positives) * (len(positives) + 1) / 2) / (len(positives) * negatives)


def bootstrap(by_session, metric, draws=BOOTSTRAP_DRAWS, seed=SEED):
    """95% interval of `metric`, resampling whole sessions with replacement."""
    rng, sessions, scores = random.Random(seed), sorted(by_session), []
    for _ in range(draws):
        score = metric([pair for _ in sessions for pair in by_session[rng.choice(sessions)]])
        if score is not None:
            scores.append(score)
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


def caught(verdict, rests):
    """A flag whose resolved citations name an owner message the label's `rests_on` names."""
    cited = {c.get("msg") for o in verdict.get("objections") or [] for c in o.get("citations") or [] if c.get("bytes")}
    return verdict["verdict"] != "accept" and bool(cited & rests)


def section(rows, classes, rests, sessions):
    verdicts = [r for r in rows if r.get("verdict") in VERDICTS]
    scored = [v for v in verdicts if classes.get(v["id"]) in POSITIVE + ("accepted",)]
    by_session = {}
    for v in scored:
        by_session.setdefault(sessions.get(v["id"], v["id"]), []).append(v)
    flags = {k: [(classes[v["id"]] in POSITIVE, v["verdict"] != "accept") for v in vs] for k, vs in by_session.items()}
    chances = {k: [(classes[v["id"]] in POSITIVE, v["p_objection"]) for v in vs if "p_objection" in v]
               for k, vs in by_session.items()}
    flat = [p for group in flags.values() for p in group]
    scores = balanced(flat)
    rate, cited = resolution(verdicts)
    losses = [v for v in scored if classes[v["id"]] == "intent_loss"]
    revealed = [v["verdict"] != "accept" for v in scored if classes[v["id"]] == "check_revealed"]
    positives = sum(1 for p, _ in flat if p)
    return {"n": len(scored), "new_info": sum(1 for v in verdicts if classes.get(v["id"]) == "new_info"),
            "positive_rate": positives / len(flat) if flat else None, "resolution": rate, "cited": cited,
            "recall": scores[1] if scores else None, "specificity": scores[2] if scores else None,
            "balanced": scores[0] if scores else None,
            "interval": bootstrap(flags, lambda pairs: (balanced(pairs) or (None,))[0]),
            "auroc": auroc([p for group in chances.values() for p in group]),
            "auroc_interval": bootstrap(chances, auroc),
            "catch": sum(1 for v in losses if caught(v, rests.get(v["id"], set()))) / len(losses) if losses else None,
            "revealed": sum(revealed) / len(revealed) if revealed else None,
            "missing": len(rows) - len(verdicts), "cost": sum(r.get("costUsd") or 0.0 for r in rows)}


def fmt(value):
    return "  n/a" if value is None else f"{value:.3f}"


def gate_line(held):
    """PASS also needs every held-out call answered: an unanswered boundary drops out of the
    score, the hard ones first."""
    rate, low = held["resolution"], (held["auroc_interval"] or (None, None))[0]
    ok = rate is not None and rate >= 0.95 and low is not None and low > 0.5 and held["missing"] == 0
    return (f"gate: {'PASS' if ok else 'FAIL'}: held-out citation resolution {fmt(rate)} (>= 0.95), "
            f"AUROC lower bound {fmt(low)} (> 0.5) and {held['missing']} calls without a verdict (0)")


def cmd_report(args):
    out = Path(args.out)
    classes, rests, source = effective_labels(out)
    boundaries_by_id = by_id(read_rows(out / "boundaries.jsonl"))
    sessions = {k: v["conversation"] for k, v in boundaries_by_id.items()}
    label_rows = read_rows(out / source)
    print(f"labels ({source}): {len(classes)} of {len(boundaries_by_id)} boundaries, "
          f"${sum(r.get('costUsd') or 0.0 for r in label_rows):.4f}; "
          f"{sum(1 for c in classes.values() if c == 'new_info')} new_info left out of scoring, "
          f"{sum(1 for r in label_rows if r.get('demoted'))} of them demoted for a rests_on that resolved to nothing")
    for key in sorted({(b["corpus"], b["split"]) for b in boundaries_by_id.values()}):
        mine = [classes[k] for k, b in boundaries_by_id.items() if (b["corpus"], b["split"]) == key and k in classes]
        scored = [c for c in mine if c != "new_info"]
        rate = sum(1 for c in scored if c in POSITIVE) / len(scored) if scored else None
        print(f"   {key[0]:8s}{key[1]:10s}{len(mine):5d} labelled, {len(mine) - len(scored)} new_info, "
              f"positive rate {fmt(rate)}")
    touched, arms = set(), []
    for path in sorted((out / "verdicts").glob("*.jsonl")):
        rows = list(by_id(read_rows(path)).values())
        arm = next((r["arm"] for r in rows if r.get("arm")), "v1")
        if any(row.get("split") == "held-out" for row in rows):
            touched.add(path.stem)
        verdicts = [r for r in rows if r.get("verdict") in VERDICTS]
        errors = [r.get("error") for r in rows if r.get("error") and r.get("verdict") not in VERDICTS]
        print(f"\n== {path.stem} ({arm}): {len(verdicts)} verdicts, {len(errors)} calls without one")
        for error in sorted(set(errors))[:5]:
            print(f"   error x{errors.count(error)}: {error}")
        print(f"   {'corpus':8s}{'split':10s}{'n':>5s}{'pos':>7s}{'new':>5s}{'cite':>7s}{'recall':>8s}{'spec':>7s}"
              f"{'bal':>7s}  95% interval   {'auroc':>6s}  95% interval   {'catch':>6s}{'check':>7s}{'cost':>9s}")
        held = None
        for split in ("fit", "held-out"):
            for corpus in sorted({r.get("corpus") for r in rows}) + ["all"]:
                part = [r for r in rows if r.get("split") == split and corpus in ("all", r.get("corpus"))]
                if not part:
                    continue
                s = section(part, classes, rests, sessions)
                if corpus == "all":
                    arms.append((rows[0].get("model") or "", rows[0].get("effort") or "", split, arm, s))
                    held = s if split == "held-out" else held
                low, high = s["interval"] or (None, None)
                a_low, a_high = s["auroc_interval"] or (None, None)
                print(f"   {corpus:8s}{split:10s}{s['n']:5d}{fmt(s['positive_rate']):>7s}{s['new_info']:5d}"
                      f"{fmt(s['resolution']):>7s}{fmt(s['recall']):>8s}{fmt(s['specificity']):>7s}"
                      f"{fmt(s['balanced']):>7s}  [{fmt(low)}, {fmt(high)}]{fmt(s['auroc']):>7s}  [{fmt(a_low)}, {fmt(a_high)}]"
                      f"{fmt(s['catch']):>7s}{fmt(s['revealed']):>7s}{s['cost']:9.4f}")
        print("   baselines: always-accept and always-flag score balanced accuracy 0.5, a constant p_objection AUROC 0.5")
        print("   " + gate_line(held or {"resolution": None, "auroc_interval": None, "missing": 0}))
    print(f"\n== arms side by side, all corpora\n   {'model':38s}{'effort':8s}{'split':10s}{'arm':6s}{'n':>5s}"
          f"{'auroc':>7s}  95% interval   {'bal':>6s}{'catch':>7s}{'check':>7s}  gate")
    for model, effort, split, arm, s in sorted(arms, key=lambda a: a[:4]):
        a_low, a_high = s["auroc_interval"] or (None, None)
        gate = gate_line(s).split(":")[1].strip() if split == "held-out" else "-"
        print(f"   {model:38s}{effort:8s}{split:10s}{arm:6s}{s['n']:5d}{fmt(s['auroc']):>7s}  [{fmt(a_low)}, {fmt(a_high)}]"
              f"{fmt(s['balanced']):>7s}{fmt(s['catch']):>7s}{fmt(s['revealed']):>7s}  {gate}")
    print("\njudge runs (model-arm-framing sha256) that have touched held-out: " + (", ".join(sorted(touched)) or "none"))
    return 0


def cmd_mark(args):
    if args.label not in CLASSES:
        print(f"refused: label must be one of {', '.join(CLASSES)}", file=sys.stderr)
        return 2
    append(Path(args.out) / "marks.jsonl", {"boundary": args.boundary, "label": args.label, "ts": int(time.time() * 1000)})
    return 0


def listing(directories):
    return sorted((str(p), p.stat().st_mtime_ns, p.stat().st_size)
                  for d in directories for p in Path(d).rglob("*") if p.is_file())


def cmd_all(args):
    """The dry pipeline end to end: every phase, both arms, then the proof that the corpus was only read."""
    if not args.dry:
        print("refused: `all` is the dry pipeline; a paid run is its phases, one at a time", file=sys.stderr)
        return 2
    specs = [spec.split(":", 1) for spec in args.corpus] or [[k, str(v)] for k, v in FIXTURES.items()]
    before = listing(d for _, d in specs)
    if not args.out:
        args.out = tempfile.mkdtemp(prefix="yi-replay-out-")
        atexit.register(shutil.rmtree, args.out, True)
    args.prompt, args.split, args.limit = None, None, None
    args.corpus = [f"{k}:{d}" for k, d in specs]
    for phase, arm in ((cmd_extract, None), (cmd_label, None), (cmd_judge, "self"), (cmd_judge, "judge"), (cmd_report, None)):
        args.arm = arm
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
    for name in ("extract", "label", "judge", "report", "mark", "all"):
        sub = commands.add_parser(name)
        sub.add_argument("--out", required=name != "all")
        if name in ("extract", "all"):
            sub.add_argument("--corpus", action="append", default=[], help="yi:<dir> or claude:<dir>; repeatable")
        if name in ("label", "judge", "all"):
            sub.add_argument("--model", required=True)
            sub.add_argument("--effort", choices=("low", "medium", "high"))
            sub.add_argument("--jobs", type=int, default=1)
            sub.add_argument("--cap-usd", type=float, default=None)
            sub.add_argument("--dry", action="store_true", help="faux only, host-built replies, no request")
        if name in ("label", "judge"):
            sub.add_argument("--split", choices=("fit", "held-out"))
            sub.add_argument("--limit", type=int, default=None)
        if name == "judge":
            sub.add_argument("--arm", choices=ARMS, required=True, help="self: the agent asked the owner's check; judge: an outside reader")
            sub.add_argument("--prompt", help="the arm's system prompt; default replay/<arm>.md")
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
        if not args.dry and not args.model.startswith("openrouter/"):
            print(f"refused: calls go straight to OpenRouter, so --model is openrouter/<id>, not {args.model}",
                  file=sys.stderr)
            return 2
        if not args.dry and not os.environ.get(KEY, "").strip():
            print(f"refused: {KEY} is unset", file=sys.stderr)
            return 2
        if not args.dry and args.cap_usd is None:
            print("refused: --cap-usd is required for a real-model run (plan law 3)", file=sys.stderr)
            return 2
    return globals()[f"cmd_{args.command}"](args)


if __name__ == "__main__":
    sys.exit(main())
