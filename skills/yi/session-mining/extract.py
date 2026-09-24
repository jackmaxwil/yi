#!/usr/bin/env python3
"""Yi session-mining extractor, schema v1 (stdlib only).

ELT: the session JSONL is immutable truth, this extractor is versioned, and
everything under --out is a disposable derived store, regenerated every sweep.
Human decisions live in marks.jsonl and are joined at derive time.

Deterministic by construction: sorted inputs, no wall clock in derived rows.
Two sweeps over the same corpus produce byte-identical stores; --selfcheck
pins that, and pins that planted fake secrets never reach an output file.
"""

import argparse
import collections
import hashlib
import itertools
import json
import math
import os
import re
import sys
import time
from pathlib import Path

import error_class

EXTRACTOR_VERSION = 6
SCHEMA_VERSION = 2

# Tool names from the crates/tools registry that cannot change the tree; the
# first call outside this set is the session's first productive action.
READ_ONLY_TOOLS = frozenset({"read", "glob", "grep"})

# One vocabulary, third use: READ_ONLY_VERBS / READ_ONLY_GIT / read_only_segment
# in crates/tools/src/builtins.rs, which the permission layer already reads. A
# verb added there and not here silently ends orientation early.
READ_ONLY_VERBS = frozenset(
    "ls cat head tail wc pwd echo printf which type file stat du df date env"
    " printenv grep rg ag find fd diff true".split()
)
READ_ONLY_GIT = frozenset(
    "log status diff show blame branch describe rev-parse ls-files shortlog".split()
)

# One vocabulary, second use: PROTECTED_CREDENTIAL_SUBPATHS in
# crates/permission/src/catastrophic.rs. A store added there and not here stops
# being redacted out of every mined artifact.
CREDENTIAL_SUBPATHS = [".ssh", ".gnupg", ".aws", ".kube", ".docker"]

# These three are hand-copied out of Rust, so --selfcheck reads the Rust back
# and compares. An asserted invariant nobody runs is how a mirror drifts.
RUST_MIRRORS = (
    ("crates/tools/src/builtins.rs", "READ_ONLY_VERBS", READ_ONLY_VERBS),
    ("crates/tools/src/builtins.rs", "READ_ONLY_GIT", READ_ONLY_GIT),
    (
        "crates/permission/src/catastrophic.rs",
        "PROTECTED_CREDENTIAL_SUBPATHS",
        frozenset(CREDENTIAL_SUBPATHS),
    ),
)

CRED_PATH_RE = re.compile(
    r"[/~]\.(?:" + "|".join(s[1:] for s in CREDENTIAL_SUBPATHS) + r")(?:[/\s\"':]|$)"
)
KEYWORD_RE = re.compile(r"(?i)(authorization|bearer|api[-_]?key|token|secret|password)")
ASSIGN_RE = re.compile(r"([A-Za-z_][A-Za-z0-9_.-]*)\s*=\s*([^\s'\"`,;)]{16,})")
LONG_TOKEN_RE = re.compile(r"[A-Za-z0-9+/_.:=-]{32,}")
DECISIVE_RE = re.compile(
    r"(?i)\b(error|errno|failed|failure|denied|not found|no such|exception|"
    r"traceback|panic|invalid|refused|timed out|cannot|unable)\b"
)
MASK = "[MASKED]"

# A uniformly random hex string samples at ~3.97 bits/char, so the 4.0 that
# reads like the natural ceiling would miss every SHA. 3.5 catches hex and
# ulids; that over-masking is the caveat the report prints.
ENTROPY_MIN = 3.5

# Provisional until fitted from real reports: a proposal sharing 60% of its
# content words with an existing doctrine window is a restatement.
DEDUPE_THRESHOLD = 0.6
DEDUPE_SOURCES = [
    "crates/runtime/src/prompts/doctrine.md",
    "crates/runtime/src/prompts/har-core.md",
    "crates/runtime/src/prompts/identity.md",
    "crates/runtime/src/prompts/orchestrate.md",
]
STOPWORDS = frozenset(
    "the a an and or but if then than that this these those is are was were be been being to of "
    "in on at by for with from as it its into not no do does done can may must should will would "
    "you your we our they them he she his her i me my one two use used using when while over".split()
)

CAVEAT = (
    "redaction caveat: the entropy rule masks any 32+ char high-entropy token, "
    "so commit SHAs and ulids in examples read as [MASKED]; a credential keyword "
    "with no separator beside it masks the rest of its line."
)


def _entropy(text):
    counts = collections.Counter(text)
    n = len(text)
    return -sum((c / n) * math.log2(c / n) for c in counts.values())


def _mixed_class(value):
    classes = (
        any(c.isupper() for c in value),
        any(c.islower() for c in value),
        any(c.isdigit() for c in value),
        any(not c.isalnum() for c in value),
    )
    return sum(classes) >= 2


def _mask_assign(match):
    name, value = match.group(1), match.group(2)
    return f"{name}={MASK}" if _mixed_class(value) else match.group(0)


def _mask_token(match):
    token = match.group(0)
    return MASK if _entropy(token) > ENTROPY_MIN else token


def redact_line(line):
    """Return the redacted line, or None when the whole line must be dropped."""
    if CRED_PATH_RE.search(line):
        return None
    home = os.environ.get("HOME")
    if home:
        line = line.replace(home, "~")
    keyword = KEYWORD_RE.search(line)
    if keyword:
        head, tail = line[: keyword.end()], line[keyword.end() :]
        # A separator binds the secret to the keyword only when nothing but
        # space or quotes stands between them: scanning on to a later URL colon
        # masked the innocuous tail and left the secret itself standing.
        sep = None
        for i, char in enumerate(tail):
            if char in ":=":
                sep = i
                break
            if char not in " \t'\"":
                break
        if sep is not None:
            line = f"{head}{tail[: sep + 1]} {MASK}"
        elif tail.strip():
            line = f"{head} {MASK}"
    line = ASSIGN_RE.sub(_mask_assign, line)
    return LONG_TOKEN_RE.sub(_mask_token, line)


def redact(text):
    kept = [redact_line(l) for l in str(text).splitlines()]
    return "\n".join(l for l in kept if l is not None)


def normalize(line):
    s = line.strip().lower()
    s = re.sub(r"'[^']*'|\"[^\"]*\"", "S", s)
    s = re.sub(r"(?:/[\w.@+-]+)+", lambda m: m.group(0).rsplit("/", 1)[-1] or "P", s)
    s = re.sub(r"\b[0-9a-f]{8,}\b", "H", s)
    s = re.sub(r"\d+", "N", s)
    return " ".join(s.split())


def fingerprint(tool, error_line):
    digest = hashlib.sha256(f"{tool}\0{normalize(error_line)}".encode())
    return digest.hexdigest()[:12]


def args_hash(arguments):
    canonical = json.dumps(arguments, sort_keys=True, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()[:12]


def session_dir_for_cwd(cwd):
    """Mirrors session_directory_name in crates/session/src/jsonl.rs."""
    trimmed = cwd[1:] if cwd[:1] in ("/", "\\") else cwd
    encoded = "".join("-" if c in "/\\:" else c for c in trimmed)
    return Path.home() / ".yi" / "sessions" / f"--{encoded}--"


def read_session(path):
    header, entries, corrupt = None, [], 0
    for line in path.read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            obj = json.loads(line)
        except ValueError:
            corrupt += 1
            continue
        if not isinstance(obj, dict):
            corrupt += 1
            continue
        if obj.get("kind") == "header":
            header = obj
        else:
            entries.append(obj)
    return header, entries, corrupt


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(b.get("text", "") for b in content if isinstance(b, dict))
    return ""


def read_only_segment(segment):
    tokens = [
        t
        for t in segment.split()
        if not (">" in t and t.endswith("/dev/null"))
    ]
    if any(">" in t or "`" in t or "$(" in t for t in tokens):
        return False
    # Rust drops only the *leading* env assignments, so `git a=b log` keeps its
    # argument and an all-assignment segment runs nothing. Filtering everywhere
    # answered the opposite on both, which is a second vocabulary, not a port.
    words = list(itertools.dropwhile(lambda word: "=" in word, tokens))
    if not words:
        return True
    verb = words[0].rsplit("/", 1)[-1]
    if verb == "git":
        return len(words) > 1 and words[1] in READ_ONLY_GIT
    return verb in READ_ONLY_VERBS


def read_only_command(command):
    """Port of read_only_segment in crates/tools/src/builtins.rs: every segment
    of the command must be read-only, so `ls && rm -rf x` is not."""
    parts = [p for chunk in re.split(r"[|;\n]", command) for p in chunk.split("&&")]
    return bool(command.strip()) and all(read_only_segment(p) for p in parts)


def call_is_read_only(name, arguments):
    """Incident: `READ_ONLY_TOOLS` screened the tool name only, so `pwd && ls -la`
    ended orientation and 6 of 9 mined sessions reported 0 read-only calls before
    their first mutation. `ipython` stays a mutation: arbitrary code has no
    deterministic screen, and guessing one is the shadow model this avoids."""
    if name in READ_ONLY_TOOLS:
        return True
    if name == "bash":
        return read_only_command(arguments.get("command") or "")
    return False


GATE_WORDS = ("cargo nextest", "cargo test", "just check", "cargo clippy",
              "pytest", "npm test", "bun test", "make test", "go test")
RED_SUMMARY = re.compile(r"\b[1-9]\d* failed\b|test result: FAILED|FAILED")
OFFER_WORDS = ("let me know", "would you like", "want me to", "if you want", "shall i")
SIGNAL_NAMES = (
    "gate_without_change", "stopped_with_open_todos", "multi_step_without_todo", "todo_stale",
    "asked_twice", "closing_offer", "flag_error", "empty_filter", "pointer_never_read",
    "chain_stop", "self_capped", "sandbox_denial_as_finding", "count_claim", "answer_shape",
    "cache_miss_streak", "done_without_check", "intercept_capped",
    "blocked_on_user_without_question", "waiting_without_block", "gate_rerun_unchanged_tree",
    "intercept_count", "intercept_max_rung", "regression_seen_red",
    "bash_timeouts", "broad_search_refused", "length_redrive", "unsourced_redrive",
    "repeat_break", "length_forced", "closed_list_nudge", "impossible_redrive", "artifact_redrive",
    "stream_retry", "kernel_dead", "module_missing", "reduced_results",
    "artifact_steer", "artifact_refused", "closure_refused", "gate_waived", "evidence_shape_refused",
    "spiral_cut", "kernel_cells", "shell_cells", "children_spawned", "readers_spawned",
    "verification_requested", "verdict_pass", "verdict_fail", "verdict_abstain",
    "verdict_escalate", "done_refused", "done_after_refusal", "verification_stale",
    "candidate_submitted", "candidate_verified", "integration_prepared", "integration_verified",
    "integration_stale", "accepted", "disposition_retained", "disposition_discarded",
    "disposition_merge_failed", "disposition_repossession_pending",
    "juror_pass", "juror_fail", "juror_abstain", "quotes_dropped",
)

# The outcome word plan.op's refusal detail names (contract.rs Outcome::Display), read back out
# of the `refusal.detail` string a `done_refused` or `verification_stale` custom entry carries.
# There is no structured outcome field on the session-visible record (plan section 6.3 commits
# the verdict to the journal, not to this entry), so the display text is the only witness.
VERDICT_OUTCOME_RE = re.compile(r": (fail|abstain|escalate)\b")


def _tokens(text):
    return set(re.findall(r"[a-z0-9]{3,}", (text or "").lower()))


def _open_items(record):
    items = []
    for phase in (record.get("list") or {}).get("phases") or []:
        for item in phase.get("items") or []:
            items.append(item)
            items.extend(item.get("children") or [])
    return items


def signals(entries):
    """Deterministic per-session facts the doctrine names; each key is an incident count
    (0 when it never happened), computed from the JSONL and nothing the model said about it."""
    out = {name: 0 for name in SIGNAL_NAMES}
    calls, results, users, assistants = {}, [], [], []
    todo_records, intercepts, custom_intercept, plan_op_records = [], [], [], []
    for entry in entries:
        if entry.get("type") == "custom":
            kind = entry.get("customType")
            if kind == "todo":
                todo_records.append(entry.get("data") or {})
            elif kind == "todo_intercept":
                custom_intercept.append(entry.get("data") or {})
            elif kind == "plan_op":
                plan_op_records.append(entry.get("data") or {})
            continue
        if entry.get("type") != "message":
            continue
        message = entry.get("message") or {}
        role = message.get("role")
        if role == "user":
            users.append(text_of(message.get("content")))
        elif role == "custom" and message.get("customType") == "todo_intercept":
            intercepts.append(message)
        elif role == "custom" and message.get("customType") == "length_redrive":
            out["length_redrive"] += 1
            if (message.get("details") or {}).get("forced"):
                out["length_forced"] += 1
            if (message.get("details") or {}).get("cut"):
                out["spiral_cut"] += 1
        elif role == "custom" and message.get("customType") == "todo_nudge" and str(message.get("content", "")).startswith("Every item is done"):
            out["closed_list_nudge"] += 1
        elif role == "custom" and message.get("customType") == "todo_nudge" and str(message.get("content", "")).startswith("None of "):
            out["artifact_steer"] += 1
        elif role == "custom" and message.get("customType") == "repeat_break":
            out["repeat_break"] += 1
        elif role == "custom" and message.get("customType") == "stream_retry":
            out["stream_retry"] += 1
        elif role == "assistant":
            turn = {"text": "", "calls": [], "usage": message.get("usage") or {}, "stop": message.get("stopReason")}
            for block in message.get("content") or []:
                if not isinstance(block, dict):
                    continue
                if block.get("type") == "text":
                    turn["text"] += block.get("text") or ""
                elif block.get("type") == "toolCall":
                    call = {"id": block.get("id"), "tool": block.get("name"), "args": block.get("arguments") or {}, "turn": len(assistants)}
                    calls[call["id"]] = call
                    turn["calls"].append(call)
            assistants.append(turn)
        elif role == "toolResult":
            results.append({"id": message.get("toolCallId"), "tool": message.get("toolName"), "text": text_of(message.get("content")), "error": bool(message.get("isError"))})
    ordered = [c for t in assistants for c in t["calls"]]
    result_of = {r["id"]: r for r in results}
    final = assistants[-1]["text"] if assistants else ""
    edits_before = 0
    source_reads = 0
    landed_since_todo = 0
    last_gate = None
    red_gates, landed_since_red = set(), False
    for call in ordered:
        tool, args = call["tool"], call["args"]
        command = args.get("command") or "" if isinstance(args, dict) else ""
        if tool == "read" and str(args.get("path", "")).endswith(".rs"):
            source_reads += 1
        gate = tool == "bash" and any(word in command for word in GATE_WORDS)
        if gate:
            if edits_before == 0 and source_reads == 0:
                out["gate_without_change"] += 1
            if last_gate == command:
                out["gate_rerun_unchanged_tree"] += 1
            last_gate = command
        mutating = tool in ("edit", "write") or (tool == "bash" and not gate and command and not read_only_command(command))
        if mutating:
            edits_before += 1
            landed_since_todo += 1
            last_gate = None
            landed_since_red = bool(red_gates)
        if tool == "todo":
            if (args.get("op") if isinstance(args, dict) else None) != "view":
                landed_since_todo = 0
        result = result_of.get(call["id"])
        if result is None:
            continue
        if gate:
            # A gate piped through `tail` reports tail's exit; the runner's own
            # summary line ("1 failed", "test result: FAILED") is the other witness.
            red = result["error"] or "exit code:" in result["text"] or bool(RED_SUMMARY.search(result["text"]))
            if red:
                red_gates.add(command)
            elif red_gates and landed_since_red:
                # Incident: a rollout ran its new test red, edited, then ran the whole
                # suite green; the exact-command match scored that as never seen red.
                out["regression_seen_red"] = 1
        if "unexpected argument" in result["text"]:
            out["flag_error"] += 1
        if "0 tests run" in result["text"]:
            out["empty_filter"] += 1
        if tool == "bash" and "&&" in command and ("exit code:" in result["text"] or "[chain stopped" in result["text"]):
            out["chain_stop"] += 1
        if tool == "bash" and isinstance(args, dict) and args.get("max_output_lines") and "lines omitted" not in result["text"]:
            out["self_capped"] += 1
        if tool == "bash" and "[timed out after" in result["text"]:
            out["bash_timeouts"] += 1
        if tool == "bash" and result["text"].startswith("[refused: "):
            out["broad_search_refused"] += 1
        if tool == "ipython":
            code = str((args or {}).get("code", "")) if isinstance(args, dict) else ""
            out["kernel_cells"] += 1
            if code.lstrip().startswith("%%bash"):
                out["shell_cells"] += 1
            if "rlm.run(" in code or "rlm(" in code:
                out["children_spawned"] += 1
                if "deny_write" in code:
                    out["readers_spawned"] += 1
        if tool == "ipython" and (result["text"].startswith("uv is required") or result["text"].startswith("no uv and no python3")):
            out["kernel_dead"] += 1
        if tool == "ipython" and "is not installed in the kernel. Run `%pip install" in result["text"]:
            out["module_missing"] += 1
        if tool == "todo" and ("done needs evidence shaped" in result["text"] or "`set` cannot close" in result["text"]):
            out["evidence_shape_refused"] += 1
        if "PermissionDenied" in result["text"] and "PermissionDenied" in final:
            out["sandbox_denial_as_finding"] += 1
        if tool == "bash" and "lines omitted:" in result["text"]:
            out["reduced_results"] += 1
        for pointer in re.findall(r"\[full output: ([^\]]+)\]", result["text"]):
            later = [c for c in ordered if c["tool"] == "read" and str((c["args"] or {}).get("path", "")) == pointer.strip()]
            if not later:
                out["pointer_never_read"] += 1
    if landed_since_todo >= 12:
        out["todo_stale"] = 1
    mutating_calls = sum(1 for c in ordered if c["tool"] in ("edit", "write") or (c["tool"] == "bash" and (c["args"] or {}).get("command") and not read_only_command(c["args"]["command"])))
    if mutating_calls >= 3 and not any(c["tool"] == "todo" for c in ordered):
        out["multi_step_without_todo"] = 1
    if todo_records:
        last = todo_records[-1]
        open_items = [i for i in _open_items(last) if i.get("state") in ("pending", "running")]
        if open_items and assistants and assistants[-1]["stop"] == "stop":
            out["stopped_with_open_todos"] = 1
        running = any(i.get("state") == "running" for i in _open_items(last))
        blocked_user = any(i.get("state") == "blocked" and i.get("on") == "user" for i in _open_items(last))
        last_line = final.strip().splitlines()[-1] if final.strip() else ""
        # Incident: three rollouts asked the key name mid-paragraph and ended on the follow-up
        # sentence; the final line alone read every one as never asked (issue #275).
        last_para = final.strip().split("\n\n")[-1] if final.strip() else ""
        asked = ("?" in last_para or any(c["tool"] == "ask_user" for c in assistants[-1]["calls"])) if assistants else False
        if running and not blocked_user and asked:
            out["waiting_without_block"] = 1
        if blocked_user and not asked:
            out["blocked_on_user_without_question"] = 1
        for record in todo_records:
            if record.get("op") == "done":
                label = record.get("label")
                items = [i for i in _open_items(record) if i.get("label") == label]
                if items and not items[0].get("evidence"):
                    out["done_without_check"] += 1
    if any(r.get("reason") == "let go" for r in custom_intercept):
        out["intercept_capped"] = 1
    out["intercept_count"] = sum(1 for r in custom_intercept if r.get("reason") == "open")
    out["intercept_max_rung"] = max((int(r.get("rung") or 0) for r in custom_intercept), default=0)
    out["unsourced_redrive"] = sum(1 for r in custom_intercept if r.get("reason") == "unsourced")
    out["impossible_redrive"] = sum(1 for r in custom_intercept if r.get("reason") == "impossible")
    out["artifact_redrive"] = sum(1 for r in custom_intercept if r.get("reason") == "artifact")
    out["artifact_refused"] = sum(1 for r in custom_intercept if r.get("reason") == "artifact_missing")
    out["closure_refused"] = sum(1 for r in custom_intercept if r.get("reason") == "closure_unrun")
    out["gate_waived"] = sum(1 for r in custom_intercept if r.get("reason") in ("artifact_waived", "closure_waived"))
    for a, b in zip(users, users[1:]):
        ta, tb = _tokens(a), _tokens(b)
        if ta and tb and len(ta & tb) / len(ta | tb) >= 0.8:
            out["asked_twice"] += 1
    if final.strip():
        tail = final.strip().splitlines()[-1].lower()
        if tail.endswith("?") or any(word in tail for word in OFFER_WORDS):
            out["closing_offer"] = 1
    seen = " ".join(r["text"] for r in results)
    for number in set(re.findall(r"(?<![\w.])\d{3,}(?![\w.])", final)):
        if number not in seen and number not in "".join(users):
            out["count_claim"] += 1
    out["answer_shape"] = len(final)
    streak = best = 0
    for turn in assistants[1:]:
        if int((turn["usage"] or {}).get("cacheRead") or 0) == 0:
            streak += 1
            best = max(best, streak)
        else:
            streak = 0
    out["cache_miss_streak"] = best
    # Record-derived, not text-derived (plan section 10.6): one `custom{plan_op}` entry per
    # applied op, oldest first, already the journal's own append order.
    frozen_at, refused_frozen = {}, {}
    for record in plan_op_records:
        op, label = record.get("op"), record.get("todo")
        # A judged item's jurors by vote (plan section 6.4), on the `done` or `done_refused`
        # record that carried its verdict; `unbacked` is the quote check dropping a juror.
        jurors = record.get("jurors") or {}
        for vote in ("pass", "fail", "abstain"):
            out[f"juror_{vote}"] += jurors.get(vote, 0)
        out["quotes_dropped"] += jurors.get("unbacked", 0)
        if op == "start" and label:
            frozen_at[label] = record.get("contract_hash")
        elif op == "verification_requested":
            out["verification_requested"] += 1
        elif op == "verification_stale":
            out["verification_stale"] += 1
        elif op == "done_refused":
            out["done_refused"] += 1
            match = VERDICT_OUTCOME_RE.search((record.get("refusal") or {}).get("detail") or "")
            if match:
                out[f"verdict_{match.group(1)}"] += 1
            if label:
                refused_frozen[label] = frozen_at.get(label)
        elif op == "done" and record.get("to") == "done" and record.get("resolution") == "verified_done":
            out["verdict_pass"] += 1
            # Same todo, a later attempt, and the criteria it was refused under are the ones
            # it passed under: a retry that fixed the product, not the contract.
            if label in refused_frozen and refused_frozen[label] is not None and refused_frozen[label] == frozen_at.get(label):
                out["done_after_refusal"] += 1
        elif op in ("candidate_submitted", "candidate_verified", "integration_prepared",
                    "integration_verified", "integration_stale"):
            out[op] += 1
        elif op == "accepted" and record.get("to") == "done" and record.get("resolution") == "verified_done":
            out["accepted"] += 1
        elif op == "disposition":
            # Externally tagged (types/plan/acceptance.rs Disposition): {"retained": {...}} and so on.
            name = next(iter(record.get("disposition") or {}), None)
            if name in ("retained", "discarded", "merge_failed", "repossession_pending"):
                out[f"disposition_{name}"] += 1
    return out


def extract_session(path, header, entries, census):
    sid = header.get("id", path.name)
    tokens = dict(input=0, output=0, cacheRead=0, cacheWrite=0, costUsd=0.0)
    by_tool = collections.Counter()
    calls, failures, denials, interrupts, compactions = [], 0, 0, 0, 0
    turns, peak, streak, streak_max = 0, 0, 0, 0
    seen_args, repeated = set(), 0
    orientation_trace, first_mutation, reads = [], None, 0
    pending, delegation, last_stop = {}, [], None
    ended_at = header.get("createdAt", 0)
    child = dict(input=0, output=0, cacheRead=0, cacheWrite=0, costUsd=0.0)
    models, providers = [], []
    error_classes = collections.Counter()

    for entry in entries:
        etype = entry.get("type") or f"kind:{entry.get('kind', '?')}"
        census["entry"][etype] += 1
        ended_at = max(ended_at, entry.get("timestamp", 0) or 0)
        if etype == "compaction":
            compactions += 1
        if etype == "custom":
            census["custom"][entry.get("customType", "?")] += 1
        if etype == "usage" and entry.get("cause") == "child_usage_attributed":
            usage = entry.get("usage") or {}
            for key in ("input", "output", "cacheRead", "cacheWrite"):
                child[key] += int(usage.get(key) or 0)
            child["costUsd"] += float((usage.get("cost") or {}).get("total") or 0.0)
            continue
        if etype != "message":
            continue
        message = entry.get("message") or {}
        role = message.get("role", "?")
        census["role"][role] += 1
        if role == "assistant":
            turns += 1
            if message.get("model"):
                models.append(message["model"])
            if message.get("provider"):
                providers.append(message["provider"])
            usage = message.get("usage") or {}
            for key in ("input", "output", "cacheRead", "cacheWrite"):
                tokens[key] += int(usage.get(key) or 0)
            tokens["costUsd"] += float((usage.get("cost") or {}).get("total") or 0.0)
            peak = max(
                peak,
                sum(
                    int(usage.get(k) or 0)
                    for k in ("input", "cacheRead", "cacheWrite")
                ),
            )
            last_stop = message.get("stopReason")
            if last_stop == "aborted":
                interrupts += 1
            for block in message.get("content") or []:
                if not isinstance(block, dict) or block.get("type") != "toolCall":
                    continue
                name = block.get("name", "?")
                arguments = block.get("arguments") or {}
                by_tool[name] += 1
                census["tool"][name] += 1
                key = (name, args_hash(arguments))
                if key in seen_args:
                    repeated += 1
                seen_args.add(key)
                index = len(calls)
                calls.append({"tool": name, "key": key, "error": None})
                pending[block.get("id")] = index
                read_only = call_is_read_only(name, arguments)
                reads += int(read_only)
                if first_mutation is None:
                    if read_only:
                        orientation_trace.append(name)
                    else:
                        first_mutation = {
                            "calls": len(orientation_trace),
                            "turns": turns - 1,
                            "tokens": sum(
                                int(usage.get(k) or 0)
                                for k in ("input", "cacheRead", "cacheWrite")
                            ),
                        }
                code = arguments.get("code") or ""
                if "rlm.run(" in code:
                    delegation.append(
                        {"callId": block.get("id"), "briefBytes": len(code)}
                    )
        elif role == "toolResult":
            name = message.get("toolName", "?")
            body = text_of(message.get("content"))
            index = pending.pop(message.get("toolCallId"), None)
            for row in delegation:
                if row.get("callId") == message.get("toolCallId"):
                    row["resultBytes"] = len(body)
            if message.get("isError"):
                failures += 1
                census["error"][name] += 1
                blame = error_class.classify(name, body, message.get("details"))
                census.setdefault("errorClass", collections.Counter())[blame] += 1
                error_classes[blame] += 1
                streak += 1
                streak_max = max(streak_max, streak)
                if body.strip().startswith("Permission denied"):
                    denials += 1
                if index is not None:
                    calls[index]["error"] = body
                    calls[index]["errorClass"] = blame
            else:
                streak = 0

    issues = failure_events(sid, calls, ended_at)
    total_calls = sum(by_tool.values())
    mu = {
        "v": SCHEMA_VERSION,
        "sessionId": sid,
        "file": path.name,
        "startedAt": header.get("createdAt", 0),
        "endedAt": ended_at,
        "model": next(iter(models), None),
        "provider": next(iter(providers), None),
        "turns": turns,
        "toolCalls": {"total": total_calls, "byTool": dict(sorted(by_tool.items()))},
        "repeatedCalls": repeated,
        "failedStreakMax": streak_max,
        "failures": failures,
        "errorClasses": dict(sorted(error_classes.items())),
        "tokens": {**tokens, "costUsd": round(tokens["costUsd"], 6)},
        "childTokens": {**child, "costUsd": round(child["costUsd"], 6)},
        "readRatio": round(reads / total_calls, 4) if total_calls else 0.0,
        "peakContextTokens": peak,
        "compactions": compactions,
        "orientation": {
            "callsBeforeFirstMutation": (first_mutation or {}).get("calls"),
            "tokensBeforeFirstMutation": (first_mutation or {}).get("tokens"),
            "turnsBeforeFirstMutation": (first_mutation or {}).get("turns"),
        },
        "delegation": {
            "children": 0,
            "briefBytes": sum(d.get("briefBytes", 0) for d in delegation),
            "resultBytes": sum(d.get("resultBytes", 0) for d in delegation),
        },
        "friction": {
            "denials": denials,
            "interrupts": interrupts,
            "errors": failures,
            "retries": repeated,
        },
        # Reserved: escalation-ladder rows land here when S3 emits them.
        "premise": {},
        "signals": signals(entries),
        "wins": bool(last_stop == "stop" and failures == 0 and interrupts == 0),
    }
    if len(set(models)) > 1:
        mu["models"] = list(dict.fromkeys(models))
    if len(set(providers)) > 1:
        mu["providers"] = list(dict.fromkeys(providers))
    orientation = {
        "v": SCHEMA_VERSION,
        "sessionId": sid,
        "trace": orientation_trace,
        "firstMutation": first_mutation,
    }
    delegation_rows = [
        {
            "v": SCHEMA_VERSION,
            "sessionId": sid,
            "briefBytes": d.get("briefBytes", 0),
            "resultBytes": d.get("resultBytes", 0),
        }
        for d in delegation
    ]
    return mu, issues, orientation, delegation_rows


def decisive_line(text):
    """The line a human would quote: a tool result is mostly stdout, and
    fingerprinting its first line clusters on banners instead of failures."""
    lines = [l.strip() for l in text.splitlines() if l.strip()]
    hit = next((l for l in lines if DECISIVE_RE.search(l)), None)
    return hit or (lines[-1] if lines else "")


def failure_events(sid, calls, ts):
    events = []
    for i, call in enumerate(calls):
        if call["error"] is None:
            continue
        first = decisive_line(redact(call["error"]))
        following = calls[i + 1] if i + 1 < len(calls) else None
        if following is None:
            action = "unresolved"
        elif following["key"] == call["key"]:
            action = "retry-success" if following["error"] is None else "unresolved"
        else:
            action = "pivot"
        events.append(
            {
                "tool": call["tool"],
                "errorClass": call["errorClass"],
                "fingerprint": fingerprint(call["tool"], first),
                "argsHash": call["key"][1],
                "resolvedInSession": action != "unresolved",
                "resolutionAction": action,
                "example": first[:200],
                "sessionId": sid,
                "ts": ts,
            }
        )
    return events


def load_marks(out_dir):
    marks = collections.defaultdict(list)
    path = out_dir / "marks.jsonl"
    if not path.exists():
        return marks
    for line in path.read_text(errors="replace").splitlines():
        if not line.strip():
            continue
        try:
            row = json.loads(line)
        except ValueError:
            continue
        marks[row.get("fingerprint")].append(row)
    return marks


def lifecycle(marks_for_fp, last_seen):
    if not marks_for_fp:
        return "NEW", None
    latest = sorted(marks_for_fp, key=lambda m: m.get("ts", 0))[-1]
    mark, ref = latest.get("mark"), latest.get("ref")
    if mark == "retired":
        return "RETIRED", ref
    if mark == "fixed":
        return ("REGRESSED" if last_seen > latest.get("ts", 0) else "FIXED"), ref
    return "CASED", ref


def build_board(events, marks):
    grouped = collections.defaultdict(list)
    for event in events:
        grouped[event["fingerprint"]].append(event)
    board = []
    for fp, rows in grouped.items():
        stamps = [r["ts"] for r in rows]
        state, ref = lifecycle(marks.get(fp, []), max(stamps))
        board.append(
            {
                "v": SCHEMA_VERSION,
                "fingerprint": fp,
                "tool": rows[0]["tool"],
                "errorClass": rows[0]["errorClass"],
                "count": len(rows),
                "sessions": sorted({r["sessionId"] for r in rows}),
                "firstSeen": min(stamps),
                "lastSeen": max(stamps),
                "state": state,
                "ref": ref,
                "resolution": dict(
                    sorted(collections.Counter(r["resolutionAction"] for r in rows).items())
                ),
                "example": rows[0]["example"],
            }
        )
    board.sort(key=lambda r: (-r["count"], r["fingerprint"]))
    return board


def write_store(out_dir, name, rows):
    lines = [json.dumps(r, sort_keys=False, separators=(",", ":")) for r in rows]
    (out_dir / name).write_text("".join(l + "\n" for l in lines))


def sweep(sessions_dir, out_dir):
    files = sorted(sessions_dir.glob("*.jsonl"))
    mu_rows, events, orientation_rows, delegation_rows = [], [], [], []
    census = {k: collections.Counter() for k in ("entry", "role", "tool", "error", "errorClass", "custom")}
    skipped, corrupt_lines, parents = [], 0, collections.Counter()
    for path in files:
        header, entries, corrupt = read_session(path)
        corrupt_lines += corrupt
        if not header or header.get("version") != 4:
            skipped.append((path.name, "no v4 header"))
            continue
        if header.get("parentSessionId"):
            parents[header["parentSessionId"]] += 1
        mu, issues, orientation, delegation = extract_session(path, header, entries, census)
        mu_rows.append(mu)
        events.extend(issues)
        orientation_rows.append(orientation)
        delegation_rows.extend(delegation)
    for mu in mu_rows:
        mu["delegation"]["children"] = parents.get(mu["sessionId"], 0)
    mu_rows.sort(key=lambda r: (r["startedAt"], r["sessionId"]))
    orientation_rows.sort(key=lambda r: r["sessionId"])
    delegation_rows.sort(key=lambda r: (r["sessionId"], r["briefBytes"]))
    board = build_board(events, load_marks(out_dir))

    out_dir.mkdir(parents=True, exist_ok=True)
    # Self-ignoring: the derived store is disposable and must never reach a
    # commit, in this repo or any other project being mined.
    (out_dir / ".gitignore").write_text("*\n")
    write_store(out_dir, "mu.jsonl", mu_rows)
    write_store(out_dir, "issues.jsonl", board)
    write_store(out_dir, "orientation.jsonl", orientation_rows)
    write_store(out_dir, "delegation.jsonl", delegation_rows)
    return {
        "files": files,
        "mu": mu_rows,
        "board": board,
        "census": census,
        "skipped": skipped,
        "corrupt": corrupt_lines,
        "orientation": orientation_rows,
        "delegation": delegation_rows,
    }


def stamp(ms):
    if not ms:
        return "?"
    return time.strftime("%Y-%m-%d", time.gmtime(ms / 1000))


def report(result, sessions_dir, out_dir):
    mu, out = result["mu"], []
    span = (
        f"{stamp(min(r['startedAt'] for r in mu))}..{stamp(max(r['endedAt'] for r in mu))}"
        if mu
        else "no sessions"
    )
    reasons = collections.Counter(r for _, r in result["skipped"])
    reason = ", ".join(f"{n} {r}" for r, n in sorted(reasons.items())) or "none"
    # Incident: `wins` read 329/364 (90%) over a corpus whose 316 single-turn
    # faux runs cannot win or lose anything; the slice that ran a tool read
    # 4/10. Every behavioural rate quotes the substantive denominator.
    worked = [r for r in mu if r["toolCalls"]["total"]]
    out.append(
        f"{len(mu)} sessions scanned, {len(result['skipped'])} skipped ({reason}), "
        f"covering {span}; {result['corrupt']} corrupt lines skipped; "
        f"{len(worked)} of {len(mu)} carried a tool call"
    )
    out.append(CAVEAT)
    out.append(f"corpus: {sessions_dir}  ->  {out_dir}")
    out.append("")
    out.append("signal census")
    for name in ("entry", "role", "tool", "error", "errorClass", "custom"):
        counts = result["census"][name]
        body = ", ".join(f"{k}={v}" for k, v in counts.most_common()) or "-"
        out.append(f"  {name:<7} {body}")
    out.append("")
    out.append("top issues")
    out.append(f"  {'fp':<13}{'tool':<9}{'n':<4}{'state':<11}{'seen':<24}example")
    for row in result["board"][:15]:
        seen = f"{stamp(row['firstSeen'])}..{stamp(row['lastSeen'])}"
        out.append(
            f"  {row['fingerprint']:<13}{row['tool']:<9}{row['count']:<4}"
            f"{row['state']:<11}{seen:<24}{row['example'][:60]}"
        )
    if not result["board"]:
        out.append("  (none)")
    out.append("")
    oriented = [r for r in worked if r["orientation"]["callsBeforeFirstMutation"] is not None]
    if oriented:
        avg = sum(r["orientation"]["callsBeforeFirstMutation"] for r in oriented) / len(oriented)
        out.append(
            f"orientation: {len(oriented)}/{len(worked)} working sessions reached a "
            f"mutation, mean {avg:.1f} read-only calls before it"
        )
    else:
        out.append("orientation: no session reached a mutation")
    briefs = result["delegation"]
    out.append(
        f"delegation: {len(briefs)} rlm.run exchanges, "
        f"{sum(r['briefBytes'] for r in briefs)} brief bytes / "
        f"{sum(r['resultBytes'] for r in briefs)} result bytes"
    )
    out.append(
        f"wins: {sum(1 for r in worked if r['wins'])}/{len(worked)} working sessions; "
        f"repeated calls: {sum(r['repeatedCalls'] for r in mu)}; "
        f"compactions: {sum(r['compactions'] for r in mu)}"
    )
    out.append("")
    out.append("signals (sessions with at least one)")
    for name in SIGNAL_NAMES:
        if name == "answer_shape":
            continue
        hit = sum(1 for r in worked if (r.get("signals") or {}).get(name))
        if hit:
            out.append(f"  {name:<34}{hit}/{len(worked)}")
    out.append("")
    out.append(f"extractor v{EXTRACTOR_VERSION}, schema v{SCHEMA_VERSION}")
    return "\n".join(out)


def content_words(text):
    words = re.findall(r"[a-z][a-z0-9'-]+", text.lower())
    return {w for w in words if w not in STOPWORDS and len(w) > 2}


def dedupe(text, root, rules_dir):
    proposal = content_words(text)
    if not proposal:
        return [], []
    scored, missing = [], []
    if not rules_dir.is_dir():
        missing.append(str(rules_dir))
    sources = [root / p for p in DEDUPE_SOURCES] + sorted(rules_dir.glob("*"))
    for source in sources:
        if not source.is_file():
            missing.append(str(source))
            continue
        lines = source.read_text(errors="replace").splitlines()
        for i in range(len(lines)):
            window = " ".join(lines[i : i + 3])
            score = len(proposal & content_words(window)) / len(proposal)
            if score > 0:
                scored.append((score, f"{source.name}:{i + 1}", window.strip()[:90]))
    scored.sort(key=lambda r: (-r[0], r[1]))
    return scored[:3], missing


def backtest(pattern, sessions_dir):
    regex = re.compile(pattern)
    hits = []
    for path in sorted(sessions_dir.glob("*.jsonl")):
        header, entries, _ = read_session(path)
        if not header:
            continue
        for entry in entries:
            message = (entry.get("message") or {}) if entry.get("type") == "message" else {}
            body = text_of(message.get("content"))
            if body and regex.search(body):
                line = next(
                    (l for l in redact(body).splitlines() if regex.search(l)), ""
                )
                hits.append((header.get("id", path.name), entry.get("seq"), line[:90]))
    return hits


def mark(out_dir, fp, state, ref):
    out_dir.mkdir(parents=True, exist_ok=True)
    row = {
        "fingerprint": fp,
        "mark": state,
        "ref": ref,
        "ts": int(time.time() * 1000),
    }
    with (out_dir / "marks.jsonl").open("a") as handle:
        handle.write(json.dumps(row) + "\n")
    return row


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--sessions", type=Path, help="session JSONL directory")
    parser.add_argument("--out", type=Path, default=Path(".yi/mining"))
    parser.add_argument("--mark", nargs=2, metavar=("FINGERPRINT", "STATE"))
    parser.add_argument("--ref", default=None, help="case path or commit for --mark")
    parser.add_argument("--dedupe", metavar="TEXT")
    parser.add_argument("--backtest", metavar="REGEX")
    parser.add_argument("--selfcheck", action="store_true")
    args = parser.parse_args(argv)

    if args.selfcheck:
        import selfcheck

        return selfcheck.selfcheck()
    if args.mark:
        fp, state = args.mark
        if state not in ("cased", "fixed", "retired"):
            parser.error("state must be cased, fixed, or retired")
        print(json.dumps(mark(args.out, fp, state, args.ref)))
        return 0
    root = Path(__file__).resolve().parents[3]
    if args.dedupe:
        scored, missing = dedupe(args.dedupe, root, Path.cwd() / ".yi" / "rules")
        for name in missing:
            print(f"source missing, not compared: {name}")
        for score, where, line in scored:
            verdict = "DROP" if score >= DEDUPE_THRESHOLD else "ok"
            print(f"{score:.2f} {verdict:<5}{where}: {line}")
        if not scored:
            print("0.00 ok    no overlap with any available source")
        return 0
    sessions = args.sessions or session_dir_for_cwd(str(Path.cwd()))
    if not sessions.is_dir():
        print(f"no session directory: {sessions}", file=sys.stderr)
        return 1
    if args.backtest:
        hits = backtest(args.backtest, sessions)
        print(f"backtest /{args.backtest}/: would fire {len(hits)} times")
        for sid, seq, line in hits[:15]:
            print(f"  {sid} seq={seq}: {line}")
        print("a rule that fires on most sessions is noise; tighten or drop it")
        return 0
    print(report(sweep(sessions, args.out), sessions, args.out))
    return 0


if __name__ == "__main__":
    sys.exit(main())
