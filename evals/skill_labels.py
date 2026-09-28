"""Label typed messages with the Yi skill they call for, the classifier's training data (#779).

    corpus  typed user messages from Claude Code transcripts and Yi sessions -> corpus.jsonl
    label   a teacher model names the one skill each message asks for, or none -> labels.jsonl
    freeze  a stratified sample for the owner to review -> frozen.csv

Everything lands under ~/.yi/skill-labels/, outside any checkout: transcripts never enter a tree.
A message is typed when a person wrote it: in Claude Code a top-level `user` entry whose content
is a string and not a command wrapper, meta or sidechain; in Yi a user message attributed `user`.
Each message is scrubbed with the session-mining redaction before it is stored or sent.
"""

import argparse
import csv
import hashlib
import json
import os
import random
import re
import sys
import urllib.request
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import record  # noqa: E402

ROOT = Path(__file__).resolve().parent.parent
OUT = Path.home() / ".yi" / "skill-labels"
TEXT_CAP = 4000
WRAPPERS = (
    "<command-",
    "<local-command",
    "<bash-",
    "<task-notification",
    "<system-reminder",
    "Caveat:",
    "[Request interrupted",
)
REMINDER = re.compile(r"<system-reminder>.*?</system-reminder>", re.S)
TEACHER = "google/gemini-2.5-flash-lite"
ENDPOINT = "https://openrouter.ai/api/v1/chat/completions"
SYSTEM = (
    "You label a user's message to a coding agent with the one method (skill) it asks the "
    "agent to follow, or none. Reply with JSON {\"skill\": \"<name>\"}, using exactly one name "
    "from the list, or \"none\" when no listed method clearly applies."
)


def skills(root=ROOT):
    """The shipped skills that declare `trigger:`, as (name, description): the classifier's
    candidates. A folded `description: >` block is joined into one line."""
    found = []
    for path in sorted((root / "skills" / "yi").glob("*/SKILL.md")):
        head = path.read_text().split("---")[1]
        fields, key = {}, None
        for line in head.splitlines():
            if re.match(r"^[a-z][a-z_-]*:", line):
                key, _, value = line.partition(":")
                fields[key] = "" if value.strip() == ">" else value.strip()
            elif key and line.startswith(" "):
                fields[key] = (fields[key] + " " + line.strip()).strip()
        if "trigger" in fields:
            found.append((fields.get("name", path.parent.name), fields.get("description", "")))
    return found


def typed_claude(path):
    """(text, loaded skill or None) for each typed message of one Claude Code transcript."""
    rows, pending = [], None
    for line in path.read_text(errors="replace").splitlines():
        try:
            entry = json.loads(line)
        except json.JSONDecodeError:
            continue
        if entry.get("type") == "user":
            content = (entry.get("message") or {}).get("content")
            if not isinstance(content, str) or entry.get("isMeta") or entry.get("isSidechain"):
                continue
            if content.lstrip().startswith(WRAPPERS):
                continue
            pending = [REMINDER.sub("", content).strip(), None]
            rows.append(pending)
        elif entry.get("type") == "assistant" and pending and pending[1] is None:
            for block in (entry.get("message") or {}).get("content") or []:
                if isinstance(block, dict) and block.get("type") == "tool_use" and block.get("name") == "Skill":
                    pending[1] = str((block.get("input") or {}).get("skill"))
                    break
    return [(text, loaded) for text, loaded in rows if text]


def typed_yi(path):
    rows = []
    for line in path.read_text(errors="replace").splitlines():
        try:
            message = (json.loads(line) or {}).get("message") or {}
        except json.JSONDecodeError:
            continue
        if message.get("role") != "user" or message.get("attribution") != "user":
            continue
        content = message.get("content")
        if isinstance(content, list):
            content = "\n".join(block.get("text", "") for block in content if isinstance(block, dict))
        if isinstance(content, str) and content.strip():
            rows.append((content, None))
    return rows


def corpus(claude_dir, yi_dir, out):
    seen, written = set(), 0
    out.parent.mkdir(parents=True, exist_ok=True)
    sources = [("claude", path, typed_claude) for path in sorted(claude_dir.glob("*/*.jsonl"))]
    sources += [("yi", path, typed_yi) for path in sorted(yi_dir.rglob("*.jsonl"))]
    with out.open("w") as sink:
        for source, path, read in sources:
            for text, loaded in read(path):
                clean = record.scrub(text.encode(errors="replace").decode())[:TEXT_CAP]
                key = message_id(text)
                if key in seen:
                    continue
                seen.add(key)
                sink.write(json.dumps({"id": key, "source": source, "text": clean, "loaded": loaded}) + "\n")
                written += 1
    return written


def message_id(text):
    """The classifier's key for a message (yi-runtime `classifier::message_id`): the first 12 hex
    of the sha256 of its bytes with ASCII whitespace collapsed and ASCII letters lowered."""
    words = re.split(rb"[ \t\n\f\r]+", text.encode(errors="surrogatepass"))
    return hashlib.sha256(b" ".join(word for word in words if word).lower()).hexdigest()[:12]


def prompt(text, candidates):
    listed = "\n".join(f"- {name}: {description}" for name, description in candidates)
    return f"Methods:\n{listed}\n\nMessage:\n<<<\n{text}\n>>>"


def ask_teacher(text, candidates, model, key):
    body = {
        "model": model,
        "temperature": 0,
        "max_tokens": 30,
        "response_format": {"type": "json_object"},
        "usage": {"include": True},
        "messages": [{"role": "system", "content": SYSTEM}, {"role": "user", "content": prompt(text, candidates)}],
    }
    request = urllib.request.Request(
        ENDPOINT,
        data=json.dumps(body).encode(),
        headers={"authorization": f"Bearer {key}", "content-type": "application/json"},
    )
    with urllib.request.urlopen(request, timeout=60) as response:
        reply = json.load(response)
    content = reply["choices"][0]["message"]["content"] or ""
    return content, float((reply.get("usage") or {}).get("cost") or 0.0)


def verdict(content, names):
    try:
        skill = json.loads(content).get("skill")
    except (json.JSONDecodeError, AttributeError):
        return "invalid"
    return skill if isinstance(skill, str) and (skill in names or skill == "none") else "invalid"


def parsed(line):
    try:
        return json.loads(line)
    except json.JSONDecodeError:
        return None


def label(corpus_path, out, model, max_usd, limit, teacher=None):
    candidates = skills()
    names = {name for name, _ in candidates}
    done, torn = set(), False
    if out.exists():
        kept = out.read_text(errors="replace")
        done = {row["id"] for row in map(parsed, kept.splitlines()) if isinstance(row, dict) and "id" in row}
        torn = bool(kept) and not kept.endswith("\n")
    if teacher is None:
        key = os.environ.get("OPENROUTER_API_KEY")
        if not key:
            sys.exit("label: export OPENROUTER_API_KEY first; this run spends money (see --max-usd)")
        teacher = lambda text: ask_teacher(text, candidates, model, key)  # noqa: E731
    spent, labelled = 0.0, 0
    with out.open("a") as sink:
        if torn:
            sink.write("\n")
        for line in corpus_path.read_text(errors="replace").splitlines():
            row = json.loads(line)
            if row["id"] in done:
                continue
            if spent >= max_usd or (limit is not None and labelled >= limit):
                break
            content, cost = teacher(row["text"])
            spent += cost
            labelled += 1
            sink.write(json.dumps({"id": row["id"], "skill": verdict(content, names), "teacher": model, "cost": cost}) + "\n")
            sink.flush()
    return labelled, spent


def freeze(corpus_path, labels_path, out, per_skill, none, seed):
    texts = {row["id"]: row for row in map(json.loads, corpus_path.read_text().splitlines())}
    by_skill = {}
    for row in map(parsed, labels_path.read_text(errors="replace").splitlines()):
        if not isinstance(row, dict) or not {"id", "skill"} <= row.keys():
            continue
        if row["skill"] != "invalid" and row["id"] in texts:
            by_skill.setdefault(row["skill"], []).append(row["id"])
    pick = random.Random(seed)
    chosen = []
    for skill, ids in sorted(by_skill.items()):
        chosen += [(skill, item) for item in pick.sample(ids, min(len(ids), none if skill == "none" else per_skill))]
    with out.open("w", newline="") as sink:
        writer = csv.writer(sink)
        writer.writerow(["id", "source", "teacher", "owner", "text"])
        for skill, item in chosen:
            row = texts[item]
            writer.writerow([item, row["source"], skill, "", " ⏎ ".join(row["text"][:300].splitlines())])
    return len(chosen)


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="verb", required=True)
    make = sub.add_parser("corpus")
    make.add_argument("--claude", type=Path, default=Path.home() / ".claude" / "projects")
    make.add_argument("--yi", type=Path, default=Path.home() / ".yi" / "sessions")
    teach = sub.add_parser("label")
    teach.add_argument("--model", default=TEACHER)
    teach.add_argument("--max-usd", type=float, default=1.0)
    teach.add_argument("--limit", type=int)
    sample = sub.add_parser("freeze")
    sample.add_argument("--per-skill", type=int, default=15)
    sample.add_argument("--none", type=int, default=60)
    sample.add_argument("--seed", type=int, default=587)
    args = parser.parse_args(argv)
    if args.verb == "corpus":
        count = corpus(args.claude, args.yi, OUT / "corpus.jsonl")
        print(f"corpus: {count} typed messages -> {OUT / 'corpus.jsonl'}")
    elif args.verb == "label":
        count, spent = label(OUT / "corpus.jsonl", OUT / "labels.jsonl", args.model, args.max_usd, args.limit)
        print(f"label: {count} labelled with {args.model}, ${spent:.4f} -> {OUT / 'labels.jsonl'}")
    else:
        count = freeze(OUT / "corpus.jsonl", OUT / "labels.jsonl", OUT / "frozen.csv", args.per_skill, args.none, args.seed)
        print(f"freeze: {count} rows for review -> {OUT / 'frozen.csv'} (fill `owner` only where you disagree)")


if __name__ == "__main__":
    main()
