"""`extract.py --selfcheck`: redaction, determinism, lifecycle and signal checks on the fixtures."""

import collections
import json
import re
import tempfile
from pathlib import Path

import error_class
from extract import (
    DEDUPE_THRESHOLD,
    MASK,
    RUST_MIRRORS,
    SIGNAL_NAMES,
    dedupe,
    mark,
    read_only_command,
    report,
    sweep,
)

PLANTS = [
    "AKIA4FAKEFAKEFAKE9XYZ",
    "sk-fake-3QpZr7Lm2Xv9Tb4Nc8Kd1Wq6",
    "Fak3Fak3Fak3Fak3F",
    "xR7pQ2mL9vB4nT6yH1kZ8sW3dF5gJ0aC",
    "a3f5c9d21b4e8f0a7c6d5e4b3a291807f6e5d4c3",
    "Hunt3rFake2",
    "T0pF4keTok",
]


def orientation_fixture(directory):
    """A session that reads the repo with bash before it writes anything —
    the shape 5 of the 10 working sessions in the 2026-08-29 corpus open with."""
    def call(cid, tool, arguments):
        return {
            "kind": "entry",
            "lane": "main",
            "type": "message",
            "message": {
                "role": "assistant",
                "usage": {"input": 100, "cacheRead": 0, "cacheWrite": 0, "output": 5},
                "stopReason": "stop",
                "content": [
                    {"type": "toolCall", "id": cid, "name": tool, "arguments": arguments}
                ],
            },
        }

    rows = [
        {"kind": "header", "version": 4, "id": "fixture-orient", "createdAt": 1780000000000},
        call("o1", "bash", {"command": "pwd && ls -la"}),
        call("o2", "bash", {"command": "git log --oneline -5"}),
        call("o3", "write", {"path": "src/lib.rs", "content": "x"}),
        call("o4", "bash", {"command": "rm -rf /tmp/scratch"}),
    ]
    idle = [
        {"kind": "header", "version": 4, "id": "fixture-idle", "createdAt": 1780000000001},
        {
            "kind": "entry",
            "lane": "main",
            "type": "message",
            "message": {"role": "assistant", "stopReason": "stop", "content": []},
        },
    ]
    directory.mkdir(parents=True, exist_ok=True)
    (directory / "orient.jsonl").write_text(
        "\n".join(json.dumps(r) for r in rows) + "\n"
    )
    (directory / "idle.jsonl").write_text("\n".join(json.dumps(r) for r in idle) + "\n")
    return directory


def model_usage_fixture(directory):
    """v4 header has no model; spend on type:usage child rows is not on the parent message."""
    directory.mkdir(parents=True, exist_ok=True)
    rows = [
        {"kind": "header", "version": 4, "id": "fixture-mu", "createdAt": 1780000000002, "cwd": "/tmp"},
        {
            "kind": "entry",
            "lane": "main",
            "type": "message",
            "message": {
                "role": "assistant",
                "model": "claude-opus-4-5",
                "provider": "anthropic",
                "usage": {"input": 10, "output": 2, "cacheRead": 0, "cacheWrite": 0, "cost": {"total": 0.01}},
                "stopReason": "stop",
                "content": [],
            },
        },
        {
            "kind": "entry",
            "lane": "main",
            "type": "message",
            "message": {
                "role": "assistant",
                "model": "z-ai/glm-5.3-flash",
                "provider": "openrouter",
                "usage": {"input": 1, "output": 1, "cacheRead": 0, "cacheWrite": 0, "cost": {"total": 0.001}},
                "stopReason": "stop",
                "content": [],
            },
        },
        {
            "kind": "record",
            "type": "usage",
            "cause": "child_usage_attributed",
            "usage": {
                "input": 104485,
                "output": 17896,
                "cacheRead": 823040,
                "cacheWrite": 0,
                "cost": {"total": 0.0247},
            },
        },
    ]
    (directory / "mu.jsonl").write_text("\n".join(json.dumps(r) for r in rows) + "\n")
    return directory


def rust_string_array(root, relative, name):
    """The literals of a Rust `const NAME: [&str; N] = [...]`. N is checked too,
    so a regex that matched half an array cannot read as agreement."""
    source = (root / relative).read_text()
    match = re.search(
        r"const %s: \[&str; (\d+)\] = \[(.*?)\];" % re.escape(name), source, re.S
    )
    assert match, f"{relative} no longer declares {name}"
    values = re.findall(r'"([^"]*)"', match.group(2))
    assert len(values) == int(match.group(1)), f"{name}: parsed {values}"
    return frozenset(values)


def check_rust_mirrors(root):
    for relative, name, mirrored in RUST_MIRRORS:
        declared = rust_string_array(root, relative, name)
        assert declared == mirrored, (
            f"{name} drifted from {relative}: rust only "
            f"{sorted(declared - mirrored)}, python only {sorted(mirrored - declared)}"
        )
    return len(RUST_MIRRORS)


def selfcheck():
    check_rust_mirrors(Path(__file__).resolve().parents[3])
    fixtures = Path(__file__).resolve().parent / "fixtures"
    with tempfile.TemporaryDirectory() as tmp:
        first, second = Path(tmp) / "a", Path(tmp) / "b"
        result = sweep(fixtures, first)
        text = report(result, fixtures, first)
        blob = text + "".join(
            p.read_text() for p in sorted(first.glob("*.jsonl"))
        )
        for plant in PLANTS:
            assert plant not in blob, f"planted secret survived redaction: {plant}"
        assert ".ssh" not in blob, "credential-store path line survived"
        assert MASK in blob, "no plant was masked; the fixture is not exercising redaction"
        assert result["mu"], "no mu rows emitted"
        assert result["board"], "no issue rows emitted"
        assert result["corrupt"] >= 1, "corrupt-line tolerance not exercised"

        again = sweep(fixtures, second)
        for name in ("mu.jsonl", "issues.jsonl", "orientation.jsonl", "delegation.jsonl"):
            assert (first / name).read_bytes() == (second / name).read_bytes(), (
                f"{name} is not byte-identical across sweeps"
            )

        fp = result["board"][0]["fingerprint"]
        last_seen = result["board"][0]["lastSeen"]
        assert result["board"][0]["state"] == "NEW"
        plant = next(r for r in result["mu"] if r["sessionId"] == "fixture-planted-0001")
        assert plant["model"] == "faux-1", plant
        assert plant["provider"] == "faux", plant
        mark(first, fp, "cased", "docs/cases/fake.md")
        assert build_board_state(fixtures, first, fp) == "CASED"
        mark(first, fp, "fixed", "deadbeef")
        assert build_board_state(fixtures, first, fp) == "FIXED"
        (second / "marks.jsonl").write_text(
            json.dumps({"fingerprint": fp, "mark": "fixed", "ref": "old", "ts": last_seen - 1})
            + "\n"
        )
        assert build_board_state(fixtures, second, fp) == "REGRESSED"

        rules = Path(tmp) / "rules"
        rules.mkdir()
        echo = "Never claim a gate is green without reading its exit code"
        (rules / "fake-doctrine.md").write_text(echo + "\n")
        scored, missing = dedupe(echo, Path(tmp), rules)
        novel, _ = dedupe("Penguins migrate across basalt tundra each equinox", Path(tmp), rules)
        assert missing, "absent dedupe sources must be named, not silently skipped"
        assert scored and scored[0][0] >= DEDUPE_THRESHOLD, f"echo scored {scored[:1]}"
        assert not novel or novel[0][0] < DEDUPE_THRESHOLD, f"novel scored {novel[:1]}"

        assert read_only_command("pwd && ls -la")
        assert read_only_command("git log --oneline -5")
        assert not read_only_command("ls && rm -rf /tmp/scratch")
        assert not read_only_command("echo hi > file")
        assert not read_only_command("")
        # The three the Rust and the port used to answer differently on.
        assert read_only_command("TERM=dumb ls -la")
        assert read_only_command("FOO=1 BAR=2")
        assert not read_only_command("git a=b log")
        oriented = sweep(
            orientation_fixture(Path(tmp) / "orient-in"), Path(tmp) / "orient-out"
        )
        pick = lambda rows: next(r for r in rows if r["sessionId"] == "fixture-orient")
        row, trace = pick(oriented["mu"]), pick(oriented["orientation"])
        assert row["orientation"]["callsBeforeFirstMutation"] == 2, row["orientation"]
        assert trace["trace"] == ["bash", "bash"], trace
        assert row["readRatio"] == 0.5, row["readRatio"]
        text = report(oriented, Path(tmp) / "orient-in", Path(tmp) / "orient-out")
        assert "1 of 2 carried a tool call" in text, text
        assert "wins: 1/1 working sessions" in text, text

        sliced = sweep(
            model_usage_fixture(Path(tmp) / "mu-in"), Path(tmp) / "mu-out"
        )
        row = next(r for r in sliced["mu"] if r["sessionId"] == "fixture-mu")
        assert row["model"] == "claude-opus-4-5", row
        assert row["provider"] == "anthropic", row
        assert row["models"] == ["claude-opus-4-5", "z-ai/glm-5.3-flash"], row
        assert row["providers"] == ["anthropic", "openrouter"], row
        assert row["childTokens"]["input"] == 104485, row["childTokens"]
        assert row["childTokens"]["costUsd"] == 0.0247, row["childTokens"]
        assert row["tokens"]["costUsd"] == 0.011, row["tokens"]

        planted = next(r for r in result["mu"] if r["sessionId"] == "fixture-planted-0001")
        assert not any(planted["signals"][n] for n in SIGNAL_NAMES if n not in ("answer_shape", "cache_miss_streak", "multi_step_without_todo", "kernel_cells", "children_spawned")), planted["signals"]
        signal_row = next(r for r in result["mu"] if r["sessionId"] == "fixture-signals")
        for name in SIGNAL_NAMES:
            if name in ("blocked_on_user_without_question", "evidence_shape_refused"):
                continue
            assert signal_row["signals"][name], f"signal {name} did not fire on its fixture"
        assert signal_row["signals"]["intercept_count"] == 1 and signal_row["signals"]["intercept_max_rung"] == 3, "a re-drive reason must not count as an open intercept"
        assert signal_row["signals"]["length_forced"] == 1 and signal_row["signals"]["length_redrive"] == 2, signal_row["signals"]
        piped_row = next(r for r in result["mu"] if r["sessionId"] == "fixture-signals-piped")
        assert piped_row["signals"]["regression_seen_red"] == 1, "a red run piped through tail must still be seen red"
        blocked_row = next(r for r in result["mu"] if r["sessionId"] == "fixture-signals-blocked")
        assert blocked_row["signals"]["blocked_on_user_without_question"] == 1, blocked_row["signals"]
        nested = next(r for r in result["mu"] if r["sessionId"] == "fixture-signals-blocked-v2")
        assert nested["signals"]["blocked_on_user_without_question"] == 1, "a format-2 blocker is nested"
        assert blocked_row["signals"]["evidence_shape_refused"] == 1, blocked_row["signals"]
        assert blocked_row["signals"]["waiting_without_block"] == 0 and "?" not in (blocked_row.get("final") or ""), "the blocked fixture asks nothing in its last paragraph"
        assert blocked_row["signals"]["waiting_without_block"] == 0, blocked_row["signals"]
        signal_text = report(result, fixtures, first)
        assert "gate_without_change" in signal_text and "asked_twice" in signal_text, signal_text

        types_src = Path(__file__).resolve().parents[3] / "crates/types/tests/fixtures/v4-golden.jsonl"
        golden_in = Path(tmp) / "golden-in"
        golden_in.mkdir()
        (golden_in / "v4-golden.jsonl").write_bytes(types_src.read_bytes())
        gold_rows = sweep(golden_in, Path(tmp) / "golden-out")["mu"]
        gold = next(r for r in gold_rows if r["sessionId"] == "fixture-a")
        assert gold["model"] == "claude-opus-4-5", gold
        assert gold["provider"] == "anthropic", gold
        assert gold["childTokens"]["input"] == 0, gold["childTokens"]

        labelled = error_class.selfcheck(fixtures / "error-classes" / "labelled.jsonl")
        blamed = sweep(error_class.session(labelled, Path(tmp) / "ec-in"), Path(tmp) / "ec-out")
        want = dict(sorted(collections.Counter(r["label"] for r in labelled).items()))
        assert blamed["mu"][0]["errorClasses"] == want, blamed["mu"][0]["errorClasses"]
        assert {r["errorClass"] for r in blamed["board"]} == set(want), blamed["board"]
    print(
        "ok   selfcheck: redaction, determinism, corrupt tolerance, lifecycle,"
        " dedupe, orientation, rust mirrors, model slice, childTokens, v4 golden, signals,"
        " error classes"
    )
    return 0


def build_board_state(fixtures, out_dir, fp):
    board = sweep(fixtures, out_dir)["board"]
    return next(r["state"] for r in board if r["fingerprint"] == fp)
