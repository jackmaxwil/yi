#!/usr/bin/env python3
"""The offline refiner for the procedural graph (D219; plan sections 9.4 to 9.8).

A person runs this; nothing in the runtime calls it. It reads proposed graph
edits as data, refuses the ones that break a structural rule before anything
runs, scores the survivors on the development tasks, gates them once on the
validation tasks (ties accepted, tokens per solved task not higher), and
remembers every rejection so the same edit is never paid for twice.

    python3 evals/graph/refine.py proposals.jsonl --runner './score.sh' \\
        --protocol tbv4-slice --model provider/model [--write]

The runner is the owner's: it is called as `<runner> <graph.json> <task>...`
and prints one JSON row per trial (`task`, `reward`, `input`, `cacheRead`,
`output`, the shape `evals/axes.py` scores). This file never starts a model.
"""
import argparse, hashlib, json, pathlib, re, shlex, subprocess, sys, tempfile, time

ROOT = pathlib.Path(__file__).resolve().parents[2]
GRAPH = ROOT / "crates/runtime/src/prompts/graph.json"
SPLIT = ROOT / "evals/levers/split.json"
REJECTED = ROOT / "evals/fixtures/graph/rejected.jsonl"

EDIT_KINDS = ("add_edge", "drop_edge", "reword")
MAX_EDITS_PER_RUN = 20
# The mirror of `crates/types/src/graph.rs`; `fixtures/graph/structural.json` holds both to it.
MAX_EDGES, MAX_OUT_EDGES, MAX_GUIDANCE_BYTES, MAX_PITFALLS, MAX_PITFALL_BYTES = 400, 8, 160, 3, 120
KINDS = ("tool", "op", "request")
RELATIONS = ("then", "instead", "before", "after_error", "after_refusal")
PREDICATES = {
    "always": "", "result_ok": "",
    "result_error": "denied not_found invalid_args aborted stale_tag noop_loop stale verdict tool_error",
    "output_capped": "", "todo_open": "", "todo_state": "running pending blocked",
    "plan_ready_nonempty": "",
    "child_state": "queued running finished failed needs_you stuck repossession_pending",
    "blocked_on": "user child external", "worktree_unmerged": "", "done_refused": "",
    "inbox_nonempty": "", "coroutine_unawaited": "", "method_awaited": "",
    "listing_name_missed": "", "session_on_disk": "",
    "session_in_memory": "",
}
EDGE_KEYS = {"from", "relation", "to", "condition", "guidance", "pitfalls", "weight"}
# A task id is letters, digits and the three joiners ids use; everything else separates two.
TOKEN = re.compile(r"[^0-9A-Za-z_.-]+")


class Refused(Exception):
    """The whole run is refused; nothing was scored."""


def conditions():
    return [f"{name}({arg})" if arg else name
            for name, args in PREDICATES.items() for arg in (args.split() or [""])]


def parses(graph):
    """What serde accepts: the shapes, the closed enums, and no unknown field."""
    def text(value):
        return isinstance(value, str)

    def count(value):
        return isinstance(value, int) and not isinstance(value, bool) and 0 <= value < 2**32

    if not isinstance(graph, dict) or set(graph) != {"version", "nodes", "edges"}:
        return False
    if not count(graph["version"]) or not all(isinstance(graph[k], list) for k in ("nodes", "edges")):
        return False
    for node in graph["nodes"]:
        if not isinstance(node, dict) or set(node) != {"id", "kind"}:
            return False
        if not text(node["id"]) or node["kind"] not in KINDS:
            return False
    for edge in graph["edges"]:
        if not isinstance(edge, dict) or not EDGE_KEYS - {"pitfalls"} <= set(edge) <= EDGE_KEYS:
            return False
        pitfalls = edge.get("pitfalls", [])
        if not all(text(edge[k]) for k in ("from", "to", "guidance")) or not count(edge["weight"]):
            return False
        if edge["relation"] not in RELATIONS or edge["condition"] not in conditions():
            return False
        if not isinstance(pitfalls, list) or not all(text(p) for p in pitfalls):
            return False
    return True


def check(graph, verbs):
    """`Graph::check`, rule for rule and in its order; the name of the first rule broken, or None."""
    if not parses(graph):
        return "parse"
    nodes = {node["id"] for node in graph["nodes"]}
    edges = graph["edges"]
    if not nodes <= set(verbs):
        return "unregistered_node"
    if len(edges) > MAX_EDGES:
        return "too_many_edges"
    reached = {node["id"] for node in graph["nodes"] if node["kind"] == "tool"}
    grew = True
    while grew:
        found = {e["to"] for e in edges if e["from"] in reached} - reached
        grew = bool(found)
        reached |= found
    for edge in edges:
        if edge["from"] not in nodes or edge["to"] not in nodes:
            return "unknown_node"
        if edge["from"] == edge["to"]:
            return "self_edge"
        if sum(1 for other in edges if other["from"] == edge["from"]) > MAX_OUT_EDGES:
            return "too_many_out_edges"
        if not 0 < len(edge["guidance"].encode()) <= MAX_GUIDANCE_BYTES:
            return "guidance_size"
        pitfalls = edge.get("pitfalls", [])
        if len(pitfalls) > MAX_PITFALLS or any(len(p.encode()) > MAX_PITFALL_BYTES for p in pitfalls):
            return "pitfalls_size"
        if edge["to"] not in reached:
            return "unreachable"
    return None


def key(edge):
    return tuple(edge.get(k) for k in ("from", "relation", "to", "condition"))


def edit_hash(edit):
    """The rationale is prose about the edit, not the edit."""
    body = json.dumps({"kind": edit.get("kind"), "edge": edit.get("edge")}, sort_keys=True)
    return hashlib.sha256(body.encode()).hexdigest()[:16]


def apply(graph, edit):
    """The candidate graph, or the name of the reason there is none."""
    edge = edit.get("edge")
    if edit.get("kind") not in EDIT_KINDS or not isinstance(edge, dict) or not edit.get("rationale"):
        return None, "malformed_edit"
    held = [index for index, other in enumerate(graph["edges"]) if key(other) == key(edge)]
    if (edit["kind"] == "add_edge") == bool(held):
        return None, "edge_exists" if held else "no_such_edge"
    edges = list(graph["edges"])
    if edit["kind"] == "reword" and edges[held[0]] == edge:
        return None, "no_change"
    if edit["kind"] == "add_edge":
        edges.append(edge)
    elif edit["kind"] == "drop_edge":
        del edges[held[0]]
    else:
        edges[held[0]] = edge
    return {"version": graph["version"] + 1, "nodes": graph["nodes"], "edges": edges}, None


def score(rows):
    """Section 9.8: tokens are `input + cacheRead + output` over every trial, per solved task."""
    solved = {row["task"] for row in rows if (row.get("reward") or 0) > 0}
    tokens = sum((row.get(k) or 0) for row in rows for k in ("input", "cacheRead", "output"))
    return {"passed": len(solved), "tokens": tokens,
            "tokensPerSolved": round(tokens / len(solved)) if solved else None}


def worse(candidate, current, guidance_bytes):
    """Ties pass. With nothing solved on either side there is no price per solved task, so
    section 9.8 stands alone: bytes that bought no pass have not paid for themselves."""
    if candidate["passed"] < current["passed"]:
        return "held_out_passes_dropped"
    if candidate["tokensPerSolved"] is None and current["tokensPerSolved"] is None:
        return "guidance_bytes_unpaid" if guidance_bytes > 0 else None
    if candidate["tokensPerSolved"] is None or (
            current["tokensPerSolved"] is not None
            and candidate["tokensPerSolved"] > current["tokensPerSolved"]):
        return "tokens_per_solved_rose"
    return None


def names(line, held_out):
    """Tokens, not substrings: a task id written with a JSON escape decodes back to its
    letters, an id a joiner ends (a sentence's full stop, a dash before a gloss) is still
    the id, and one inside a longer word is not."""
    try:
        text = json.dumps(json.loads(line), ensure_ascii=False)
    except json.JSONDecodeError:
        text = line
    return sorted({token.strip(".-") for token in TOKEN.split(text)} & set(held_out))


def read_proposals(path, split):
    """Refused whole: a proposal that names a held-out task was written with the answer in view."""
    held_out = split["validation"] + split["final"]
    edits = []
    for number, line in enumerate(pathlib.Path(path).read_text().splitlines(), 1):
        if not line.strip():
            continue
        named = names(line, held_out)
        if named:
            raise Refused(f"{path}:{number} names the held-out task {named[0]!r}")
        edits.append(json.loads(line))
    if len(edits) > MAX_EDITS_PER_RUN:
        raise Refused(f"{path} holds {len(edits)} edits; a run takes at most {MAX_EDITS_PER_RUN}")
    return edits


def refine(graph, edits, split, run, config, rejected_path=REJECTED, verbs=None, now=time.time):
    """One verdict per edit. `run(graph, tasks)` is the only thing that costs anything, and an
    edit reaches it only after the structural checks and the rejection memory let it through."""
    rejected_path = pathlib.Path(rejected_path)
    memory = [json.loads(line) for line in rejected_path.read_text().splitlines() if line.strip()] \
        if rejected_path.exists() else []
    verbs = verbs if verbs is not None else {node["id"] for node in graph["nodes"]}
    current, verdicts = None, []

    def reject(edit, reason, scores=None):
        row = {"editHash": edit_hash(edit), "baseVersion": graph["version"], **config,
               "reason": reason, "scores": scores, "at": int(now())}
        memory.append(row)
        with rejected_path.open("a") as sink:
            sink.write(json.dumps(row, sort_keys=True) + "\n")
        return {"editHash": row["editHash"], "verdict": "rejected", "reason": reason, "scores": scores}

    for edit in edits:
        same = [row for row in memory if row["editHash"] == edit_hash(edit)]
        banned = [row for row in same if all(
            row.get(k) == v for k, v in {"baseVersion": graph["version"], **config}.items())]
        if banned:
            verdicts.append({"editHash": edit_hash(edit), "verdict": "skipped",
                             "reason": f"already_rejected:{banned[0]['reason']}", "scores": None})
            continue
        candidate, reason = apply(graph, edit)
        reason = reason or check(candidate, verbs)
        if reason:
            verdicts.append(reject(edit, reason))
            continue
        if current is None:
            current = {"fit": score(run(graph, split["development"])),
                       "heldOut": score(run(graph, split["validation"]))}
        scores = {"base": current, "fit": score(run(candidate, split["development"])),
                  "guidanceBytes": sum(len(e["guidance"].encode()) for e in candidate["edges"])
                  - sum(len(e["guidance"].encode()) for e in graph["edges"]),
                  "earlierRejections": len(same)}
        if scores["fit"]["passed"] < current["fit"]["passed"]:
            verdicts.append(reject(edit, "fit_passes_dropped", scores))
            continue
        scores["heldOut"] = score(run(candidate, split["validation"]))
        reason = worse(scores["heldOut"], current["heldOut"], scores["guidanceBytes"])
        if reason:
            verdicts.append(reject(edit, reason, scores))
            continue
        graph, current = candidate, {"fit": scores["fit"], "heldOut": scores["heldOut"]}
        verdicts.append({"editHash": edit_hash(edit), "verdict": "promoted", "reason": None, "scores": scores})
    return graph, verdicts


def dump(graph):
    """One node and one edge per line, so a promoted edit is a one-line diff."""
    def rows(items):
        return ",\n".join("  " + json.dumps(item, ensure_ascii=False) for item in items)
    return (f'{{\n "version": {graph["version"]},\n "nodes": [\n{rows(graph["nodes"])}\n ],\n'
            f' "edges": [\n{rows(graph["edges"])}\n ]\n}}\n')


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("proposals")
    parser.add_argument("--runner", required=True, help="the owner's scoring command; this file starts no model")
    parser.add_argument("--protocol", required=True)
    parser.add_argument("--model", required=True)
    parser.add_argument("--write", action="store_true", help="write the promoted graph back to graph.json")
    args = parser.parse_args(argv)

    def run(graph, tasks):
        with tempfile.NamedTemporaryFile("w", suffix=".json") as sink:
            sink.write(dump(graph))
            sink.flush()
            done = subprocess.run([*shlex.split(args.runner), sink.name, *tasks],
                                  capture_output=True, text=True, check=True)
        return [json.loads(line) for line in done.stdout.splitlines() if line.strip()]

    split = json.loads(SPLIT.read_text())
    try:
        edits = read_proposals(args.proposals, split)
    except Refused as refusal:
        print(f"refused: {refusal}", file=sys.stderr)
        return 2
    graph, verdicts = refine(json.loads(GRAPH.read_text()), edits, split, run,
                             {"protocol": args.protocol, "model": args.model})
    for verdict in verdicts:
        print(json.dumps(verdict, sort_keys=True))
    if args.write and any(v["verdict"] == "promoted" for v in verdicts):
        GRAPH.write_text(dump(graph))
        # The Rust goldens pin the bytes today's graph renders; a promotion is a prompt change.
        print(f"wrote {GRAPH.relative_to(ROOT)}: run `cargo test -p yi-runtime --test integration affordance::`"
              " and re-pin the goldens deliberately before committing", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
