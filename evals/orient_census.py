"""Read-only census of route telemetry and get_context packets in session files.

Answers two open TODOS rows with counts instead of opinion: `P4` (route weights
from data) needs labelled route rows, `P13` (which orientation layers earn their
bytes) needs packets that were actually returned. Neither constant moves until
the readiness thresholds below are cleared by a real corpus.

The census prints counts and closed-vocabulary labels only. No prompt text, no
packet body, no path from the corpus ever reaches stdout, which is what keeps a
sweep over somebody's live sessions safe to paste into a report.

    python3 evals/orient_census.py [--corpus DIR] [--selftest]
"""

import argparse
import collections
import glob
import json
import os
import pathlib
import sys
import tempfile

CENSUS_VERSION = 2

# Proposed, not fitted. P4's own TODOS row asks for "a few hundred sessions",
# and a logistic fit with zero positive labels fits nothing at all; P13 needs
# enough packets that a layer's absence is a rate rather than an anecdote.
P4_MIN_ROUTE_ROWS = 200
P4_MIN_ESCALATIONS = 20
P13_MIN_PACKETS = 50

# Incident: the 2026-08-29 corpus reported 39 route rows and read as merely
# undersampled, while `repo_dirty` was false and `named_paths` zero in every
# one of them. A row count hides a degenerate feature matrix; a fit needs
# features that vary, so readiness counts the ones that do.
P4_MIN_LIVE_FEATURES = 5

RECORD_ENTRY = "ext_record"
ORIENT_TOOL = "get_context"
PACKET_HEADER = "# orientation packet"

# The score components crates/runtime/src/ext/orchestrate.rs persists. A row
# missing one of these predates the widening and cannot carry a fit.
FIT_FEATURES = (
    "score",
    "words",
    "enums",
    "imperatives",
    "questions",
    "and_count",
    "fenced",
    "named_paths",
    "repo_dirty",
)

# The layer names crates/tools/src/orient.rs emits, in packet order. A rename
# there turns every layer "unknown" here, and the selftest goes red for it.
PACKET_LAYERS = (
    "grid roots",
    "symbol neighborhood",
    "file skeletons",
    "git change heat",
    "gate commands",
    "prior issues",
)

LAYER_STATES = ("present", "absent", "truncated")


def rows_of(path):
    """Every JSON line, skipping the partial tail a live session is mid-write on."""
    try:
        handle = open(path, encoding="utf-8", errors="replace")
    except OSError:
        return
    with handle:
        for line in handle:
            line = line.strip()
            if not line:
                continue
            try:
                row = json.loads(line)
            except ValueError:
                continue
            if isinstance(row, dict):
                yield row


def text_of(content):
    if isinstance(content, str):
        return content
    if isinstance(content, list):
        return "\n".join(
            block.get("text", "")
            for block in content
            if isinstance(block, dict) and block.get("type") == "text"
        )
    return ""


def packet_layers(text):
    """Layer -> state, read from the packet's own self-description.

    orient.rs already names present/absent/truncated per layer in its output, so
    the transcript is the record and P13 needs no new telemetry in the binary.
    """
    states = {}
    for chunk in text.split("\n## ")[1:]:
        name, _, body = chunk.partition("\n")
        name = name.strip()
        if name not in PACKET_LAYERS:
            states.setdefault("unknown", "present")
            continue
        body = body.strip()
        if body.startswith("absent:"):
            states[name] = "absent"
        elif "[%s truncated at" % name in body:
            states[name] = "truncated"
        else:
            states[name] = "present"
    return states


def scan(files):
    result = {
        "files": len(files),
        "with_records": 0,
        "key_rows": collections.Counter(),
        "route_labels": collections.Counter(),
        "attach_signals": collections.Counter(),
        "confusion": collections.Counter(),
        "feature_coverage": collections.Counter(),
        "feature_values": collections.defaultdict(set),
        "full_feature_rows": 0,
        "packets": 0,
        "packet_headers": collections.Counter(),
        "layers": collections.Counter(),
    }
    for path in files:
        routes, signals, saw = [], [], False
        for row in rows_of(path):
            if row.get("kind") != "entry":
                continue
            if row.get("customType") == RECORD_ENTRY:
                data = row.get("data") or {}
                key, value = data.get("key"), data.get("value")
                if not isinstance(value, dict):
                    continue
                saw = True
                result["key_rows"][str(key)] += 1
                if key == "route":
                    routes.append(value)
                    result["route_labels"][str(value.get("route"))] += 1
                    present = [name for name in FIT_FEATURES if name in value]
                    result["feature_coverage"].update(present)
                    for name in present:
                        result["feature_values"][name].add(repr(value[name]))
                    if len(present) == len(FIT_FEATURES):
                        result["full_feature_rows"] += 1
                elif key == "orchestrate_attached":
                    signals.append(str(value.get("signal")))
                    result["attach_signals"][str(value.get("signal"))] += 1
                continue
            message = row.get("message")
            if not isinstance(message, dict):
                continue
            if message.get("role") != "toolResult":
                continue
            if message.get("toolName") != ORIENT_TOOL:
                continue
            body = text_of(message.get("content"))
            if not body.startswith(PACKET_HEADER):
                continue
            result["packets"] += 1
            head = body.splitlines()[1:2]
            complete = bool(head) and head[0].strip() == "COMPLETE"
            result["packet_headers"]["COMPLETE" if complete else "PARTIAL"] += 1
            for name, state in packet_layers(body).items():
                result["layers"][(name, state)] += 1
        if saw:
            result["with_records"] += 1
        if routes:
            # The P4 label: a session whose first route was not complex but
            # which later escalated on a non-prefilter signal was under-routed.
            first = str(routes[0].get("route"))
            escalated = any(signal != "prefilter" for signal in signals)
            result["confusion"][(first, escalated)] += 1
    return result


def escalations(result):
    return sum(count for (_, escalated), count in result["confusion"].items() if escalated)


def live_features(result):
    """The features that actually vary. A constant column carries no weight,
    so it is sample shape and not sample size that blocks the fit."""
    return sorted(
        name for name in FIT_FEATURES if len(result["feature_values"].get(name, ())) > 1
    )


def report(result, corpus):
    out = [
        "orient census v%d over %s" % (CENSUS_VERSION, corpus),
        "session files: %d; with ext_record: %d"
        % (result["files"], result["with_records"]),
        "rows per key: %s" % dict(sorted(result["key_rows"].items())),
        "route labels: %s" % dict(sorted(result["route_labels"].items())),
        "attach signals: %s" % dict(sorted(result["attach_signals"].items())),
        "route rows carrying every fit feature: %d of %d"
        % (result["full_feature_rows"], result["key_rows"].get("route", 0)),
        "feature coverage: %s" % dict(sorted(result["feature_coverage"].items())),
    ]
    out.append("first-route x later-escalation:")
    for (label, escalated), count in sorted(result["confusion"].items()):
        out.append("  %-10s escalated=%-5s %d" % (label, escalated, count))
    out.append(
        "get_context packets: %d %s"
        % (result["packets"], dict(sorted(result["packet_headers"].items())))
    )
    for name in PACKET_LAYERS + ("unknown",):
        counts = {
            state: result["layers"].get((name, state), 0)
            for state in LAYER_STATES
            if result["layers"].get((name, state), 0)
        }
        if counts:
            out.append("  layer %-20s %s" % (name, counts))
    route_rows = result["key_rows"].get("route", 0)
    escalated = escalations(result)
    live = live_features(result)
    constant = [name for name in FIT_FEATURES if name not in live]
    out.append("features that vary: %s" % (", ".join(live) or "none"))
    out.append("features constant across every row: %s" % (", ".join(constant) or "none"))
    p4 = (
        route_rows >= P4_MIN_ROUTE_ROWS
        and escalated >= P4_MIN_ESCALATIONS
        and len(live) >= P4_MIN_LIVE_FEATURES
    )
    p13 = result["packets"] >= P13_MIN_PACKETS
    out.append(
        "fit readiness: P4 %s (%d/%d rows, %d/%d escalations, %d/%d varying features)"
        " · P13 %s (%d/%d packets)"
        % (
            "READY" if p4 else "not ready",
            route_rows,
            P4_MIN_ROUTE_ROWS,
            escalated,
            P4_MIN_ESCALATIONS,
            len(live),
            P4_MIN_LIVE_FEATURES,
            "READY" if p13 else "not ready",
            result["packets"],
            P13_MIN_PACKETS,
        )
    )
    return "\n".join(out)


PLANTED = "sk-live-CENSUSPLANT0000000000000000"

FIXTURE_ESCALATED = [
    {"kind": "header", "version": 4, "id": "census-a", "createdAt": 1},
    {
        "kind": "entry",
        "lane": "main",
        "type": "custom",
        "customType": RECORD_ENTRY,
        "data": {
            "key": "route",
            "value": {
                "route": "one_shot",
                "words": 6,
                "repo_dirty": False,
                "named_paths": 0,
                "score": -3,
                "enums": 0,
                "imperatives": 0,
                "questions": 1,
                "and_count": 0,
                "fenced": False,
            },
        },
    },
    {
        "kind": "entry",
        "lane": "main",
        "type": "custom",
        "customType": RECORD_ENTRY,
        "data": {"key": "orchestrate_attached", "value": {"signal": "files_matched"}},
    },
    {
        "kind": "entry",
        "lane": "main",
        "type": "message",
        "message": {
            "role": "toolResult",
            "toolCallId": "c1",
            "toolName": ORIENT_TOOL,
            "content": [
                {
                    "type": "text",
                    "text": (
                        "# orientation packet\nPARTIAL - 5 of 6 layers\n"
                        "\n## grid roots\ncrates/runtime\n"
                        "\n## symbol neighborhood\nabsent: no symbol argument was given\n"
                        "\n## file skeletons\nsrc/lib.rs\n"
                        "  pub fn key() -> &str { \"%s\" }\n"
                        "\n[file skeletons truncated at 4000 bytes]\n"
                        "\n## git change heat\n   4  src/lib.rs\n"
                        "\n## gate commands\njust check  (justfile)\n"
                        "\n## prior issues\nabc123  a thing\n" % PLANTED
                    ),
                }
            ],
        },
    },
]

FIXTURE_QUIET = [
    {"kind": "header", "version": 4, "id": "census-b", "createdAt": 2},
    {
        "kind": "entry",
        "lane": "main",
        "type": "custom",
        "customType": RECORD_ENTRY,
        "data": {
            "key": "route",
            "value": {"route": "one_shot", "words": 4, "repo_dirty": False, "named_paths": 0},
        },
    },
]


DEGENERATE_SESSIONS = 20
DEGENERATE_ROUTE_ROWS = 10


def write_degenerate(directory):
    """A corpus that clears both count gates with every feature column constant:
    the shape the 2026-08-29 incident would have called READY on row count."""
    directory.mkdir(parents=True, exist_ok=True)
    value = dict({name: 0 for name in FIT_FEATURES}, route="one_shot")
    files = []
    for index in range(DEGENERATE_SESSIONS):
        rows = [
            {"kind": "header", "version": 4, "id": "census-d%d" % index, "createdAt": 3}
        ]
        rows += [
            {
                "kind": "entry",
                "lane": "main",
                "type": "custom",
                "customType": RECORD_ENTRY,
                "data": {"key": "route", "value": value},
            }
        ] * DEGENERATE_ROUTE_ROWS
        rows.append(
            {
                "kind": "entry",
                "lane": "main",
                "type": "custom",
                "customType": RECORD_ENTRY,
                "data": {"key": "orchestrate_attached", "value": {"signal": "files_matched"}},
            }
        )
        path = directory / ("census-d%d.jsonl" % index)
        path.write_text("\n".join(json.dumps(row) for row in rows) + "\n")
        files.append(str(path))
    return files


def write_fixtures(directory):
    first = directory / "census-a.jsonl"
    first.write_text("\n".join(json.dumps(row) for row in FIXTURE_ESCALATED) + "\n")
    second = directory / "census-b.jsonl"
    # A live session's last line is half-written; the census must skip it.
    second.write_text(
        "\n".join(json.dumps(row) for row in FIXTURE_QUIET) + '\n{"kind": "ent\n'
    )
    return [str(first), str(second)]


def selftest():
    with tempfile.TemporaryDirectory() as scratch:
        return _selftest(pathlib.Path(scratch))


def _selftest(directory):
    files = write_fixtures(directory)
    result = scan(files)
    assert result["files"] == 2, result["files"]
    assert result["with_records"] == 2, result["with_records"]
    assert result["key_rows"]["route"] == 2, result["key_rows"]
    assert result["key_rows"]["orchestrate_attached"] == 1, result["key_rows"]
    assert result["full_feature_rows"] == 1, result["full_feature_rows"]
    assert result["confusion"][("one_shot", True)] == 1, result["confusion"]
    assert result["confusion"][("one_shot", False)] == 1, result["confusion"]
    assert escalations(result) == 1, result["confusion"]
    assert result["packets"] == 1, result["packets"]
    assert result["packet_headers"]["PARTIAL"] == 1, result["packet_headers"]
    assert result["layers"][("grid roots", "present")] == 1, result["layers"]
    assert result["layers"][("symbol neighborhood", "absent")] == 1, result["layers"]
    assert result["layers"][("file skeletons", "truncated")] == 1, result["layers"]
    assert result["layers"][("prior issues", "present")] == 1, result["layers"]
    # Two rows whose repo_dirty and named_paths never differ: the fit has one
    # live column, which no row count fixes.
    assert live_features(result) == ["words"], live_features(result)
    text = report(result, "fixtures")
    assert PLANTED not in text, "the census leaked a packet body"
    assert (
        "features constant across every row: score, enums, imperatives, questions,"
        " and_count, fenced, named_paths, repo_dirty" in text
    ), text
    assert "not ready" in text, text

    # The live-features gate on its own terms: rows and escalations both over
    # threshold, so only the varying-feature count can still refuse.
    degenerate = scan(write_degenerate(directory / "degenerate"))
    assert degenerate["key_rows"]["route"] >= P4_MIN_ROUTE_ROWS, degenerate["key_rows"]
    assert escalations(degenerate) >= P4_MIN_ESCALATIONS, degenerate["confusion"]
    assert live_features(degenerate) == [], live_features(degenerate)
    text = report(degenerate, "fixtures")
    assert "P4 not ready" in text, text
    assert "0/%d varying features" % P4_MIN_LIVE_FEATURES in text, text
    return 23


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--corpus", default=os.path.expanduser("~/.yi/sessions"))
    parser.add_argument("--selftest", action="store_true")
    args = parser.parse_args(argv)
    if args.selftest:
        print("ok   orient_census (%d checks)" % selftest())
        return 0
    files = sorted(glob.glob(os.path.join(args.corpus, "*", "*.jsonl")))
    print(report(scan(files), args.corpus))
    return 0


if __name__ == "__main__":
    sys.exit(main())
