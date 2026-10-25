#!/usr/bin/env python3
"""Prefill the PR narrative's mechanical halves from the diff against main.

The prose sections stay the author's; only the counted ones are filled, because
a hand-typed net-LOC line is an estimate and the growth budget prices the
measured number. Prints to stdout — piping is the caller's business.
"""
import hashlib
import pathlib
import re
import subprocess
import sys
from collections import defaultdict

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent / "guardrails"))
from _common import base_commit  # noqa: E402

ROOT = pathlib.Path(__file__).resolve().parents[1]
GROWTH = ROOT / "scripts/guardrails/check_growth.py"


def git(*args):
    return subprocess.run(
        ("git", "-C", str(ROOT)) + args, capture_output=True, text=True, check=False
    )


def diff_stats(base):
    """(path, added, removed) per file; renames are split so the LOC count is honest."""
    out = git("diff", "--numstat", "--no-renames", base, "HEAD")
    rows = []
    for line in out.stdout.splitlines():
        parts = line.split("\t")
        if len(parts) != 3:
            continue
        added, removed, path = parts
        if added == "-" or removed == "-":
            rows.append((path, 0, 0))
            continue
        rows.append((path, int(added), int(removed)))
    return rows


def area(path):
    parts = path.split("/")
    if len(parts) > 1 and parts[0] == "crates":
        return "crates/" + parts[1]
    return parts[0] if len(parts) > 1 else path


def is_src(path):
    parts = path.split("/")
    return (
        len(parts) > 3
        and parts[0] == "crates"
        and parts[2] == "src"
        and path.endswith(".rs")
    )


def growth_verdict():
    """The gate's own verdict on the same fork-point number, memo included."""
    if not GROWTH.exists():
        return f"growth gate: not present in this tree ({GROWTH.relative_to(ROOT)})"
    run = subprocess.run(
        [sys.executable, str(GROWTH)], cwd=ROOT, capture_output=True, text=True, check=False
    )
    text = (run.stdout + run.stderr).strip() or "(no output)"
    return "growth gate:\n\n```\n" + text + "\n```"


def tally(rows):
    """Per-area file counts, the src-only slice, and the net the growth budget prices.
    The table printed under a net must sum to it, or the PR argues one number and the
    gate charges another."""
    by_area = defaultdict(lambda: [0, 0, 0])
    src_by_area = defaultdict(lambda: [0, 0])
    src_net = 0
    for path, added, removed in rows:
        stat = by_area[area(path)]
        stat[0] += 1
        stat[1] += added
        stat[2] += removed
        if is_src(path):
            src = src_by_area[area(path)]
            src[0] += added
            src[1] += removed
            src_net += added - removed
    return by_area, src_by_area, src_net


def section(title, body):
    return f"## {title}\n\n{body}\n"


SURFACE_LOCK = "scripts/guardrails/baselines/tool_surface.json"
# Only `--update` writes the lock, so one missing from the tree was deleted, not seeded.
LOCK_MISSING = (f"{SURFACE_LOCK} is missing; `check_request_budget.py --update` writes it,"
                " committed alone as a Ratchet")


def surface_entry(text):
    """A lock value: the text and its hash. A hash alone cannot tell a cut from a rewrite,
    and the old text is in no git object but this lock."""
    return {"hash": hashlib.sha256(text.encode()).hexdigest(), "text": text}


def sentences(text):
    return re.split(r"(?<=[.!?])\s+", " ".join(text.split()))


def trimmed(was, now):
    """A pure cut: shorter, and every sentence of the new text is one of the old text's, in
    order. Cutting inside a sentence is a rewrite: `does not carry over` minus `not` is shorter
    and made of the old words, and says the opposite. A lock written before the text was kept
    (a bare hash) cannot show a cut."""
    if "text" not in was or "text" not in now:
        return False
    old = iter(sentences(was["text"]))
    return len(now["text"].encode()) < len(was["text"].encode()) and all(s in old for s in sentences(now["text"]))


def surface_diff(was, now):
    """(added, reworded, trimmed) lock keys between two surfaces: reworded is any moved key
    that is not a pure cut, a growth included. A removal makes no new claim and asks nothing,
    so removed keys are not reported."""
    entry = lambda value: value if isinstance(value, dict) else {"hash": value}
    added = sorted(key for key in now if key not in was)
    moved = [key for key in sorted(now) if key in was and entry(was[key])["hash"] != entry(now[key])["hash"]]
    cut = [key for key in moved if trimmed(entry(was[key]), entry(now[key]))]
    return added, [key for key in moved if key not in cut], cut


def surface_delta(base):
    """(added, reworded) keys in the tool-surface lock between the merge base and the
    working tree; a trimmed key owes nothing. A lock missing at base is being
    seeded and asks nothing."""
    import json

    before = git("show", f"{base}:{SURFACE_LOCK}")
    if before.returncode != 0:
        return [], []
    now_path = ROOT / SURFACE_LOCK
    if not now_path.exists():
        raise SystemExit(LOCK_MISSING)
    added, reworded, _ = surface_diff(json.loads(before.stdout), json.loads(now_path.read_text()))
    return added, reworded


def surface_skeletons(added, changed):
    """The sections a surface change owes (D188), printed as header row and
    separator with the columns in a comment and no data row, so an unfilled
    skeleton fails check_pr_metadata.py on purpose."""
    keys = ", ".join(changed + added)
    out = []
    if changed:
        out.append(section(
            "Claims ledger",
            f"<!-- owed: the PR changes {keys}, which the model reads. "
            "claim · check · result -->\n\n"
            "| claim | check | result |\n|---|---|---|",
        ))
    if added:
        if not changed:
            out.append(section(
                "Claims ledger",
                f"<!-- owed: the PR adds {keys} to the surface the model reads. "
                "claim · check · result -->\n\n"
                "| claim | check | result |\n|---|---|---|",
            ))
        out.append(section(
            "Neighbour matrix",
            "<!-- owed: a new tool is judged against its neighbours — "
            "read, grep, glob, edit, write, bash hints, kernel, TUI, prompts. "
            "neighbour · overlap · why this one -->\n\n"
            "| neighbour | overlap | why this one |\n|---|---|---|",
        ))
        out.append(section(
            "Dogfood",
            "<!-- owed: the census bucket the tool serves and the replay's "
            "before/after. bucket · run · result -->\n\n"
            "| bucket | run | result |\n|---|---|---|",
        ))
    return out


def selfcheck():
    """A path landing in the wrong bucket misreports net src LOC, which is the
    one number the growth budget prices."""
    cases = [
        ("crates/runtime/src/wiring.rs", "crates/runtime", True),
        ("crates/runtime/tests/plan_e2e.rs", "crates/runtime", False),
        ("crates/runtime/src/advisor/mod.rs", "crates/runtime", True),
        ("crates/types/Cargo.toml", "crates/types", False),
        ("crates/tui/src/render.py", "crates/tui", False),
        ("docs/ARCHITECTURE.md", "docs", False),
        ("justfile", "justfile", False),
    ]
    for path, want_area, want_src in cases:
        assert area(path) == want_area, f"{path}: area {area(path)} != {want_area}"
        assert is_src(path) is want_src, f"{path}: is_src {is_src(path)} != {want_src}"
    rows = [
        ("crates/runtime/src/wiring.rs", 40, 5),
        ("crates/types/src/config.rs", 3, 1),
        ("crates/runtime/tests/plan_e2e.rs", 200, 0),
        ("docs/ARCHITECTURE.md", 900, 4),
    ]
    by_area, src_by_area, src_net = tally(rows)
    assert src_net == 37, f"net src {src_net} != 37"
    total = sum(added - removed for added, removed in src_by_area.values())
    assert total == src_net, f"src table sums to {total}, printed under {src_net}"
    assert set(src_by_area) == {"crates/runtime", "crates/types"}, sorted(src_by_area)
    assert by_area["crates/runtime"] == [2, 240, 5], by_area["crates/runtime"]
    # The surface delta: a lock absent at base is the seeding commit asking
    # nothing (surface_delta's early return); a removal-only delta owes nothing.
    assert surface_diff({"tool:read": "a"}, {"tool:read": "a"}) == ([], [], [])
    assert surface_diff({"tool:read": "a"}, {"tool:read": "a", "tool:foo": "b"}) == (["tool:foo"], [], [])
    assert surface_diff({"tool:read": "a"}, {"tool:read": "z"}) == ([], ["tool:read"], [])
    cut = {"tool:read": surface_entry("One. Two is not three.")}
    assert surface_diff(cut, {"tool:read": surface_entry("Two is not three.")}) == ([], [], ["tool:read"]), "a sentence cut"
    assert surface_diff(cut, {"tool:read": surface_entry("One. Two is three.")}) == ([], ["tool:read"], []), "a cut inside a sentence"
    assert surface_diff({"tool:read": "a", "tool:gone": "b"}, {"tool:read": "a"}) == ([], [], [])
    # A lock still holding bare hashes reads the same as its rewrite with the text kept.
    assert surface_diff({"tool:read": surface_entry("a b")["hash"]}, {"tool:read": surface_entry("a b")}) == ([], [], [])
    print("ok   pr_body selfcheck")


def main():
    if "--selfcheck" in sys.argv[1:]:
        selfcheck()
        return 0
    ref, base = base_commit()
    if base is None:
        print("pr-body: no origin/main or main to diff against", file=sys.stderr)
        return 1
    rows = diff_stats(base)
    if not rows:
        print(f"pr-body: no changes against {ref}", file=sys.stderr)
        return 1

    by_area, src_by_area, src_net = tally(rows)

    ui = [
        p
        for p, _, _ in rows
        if p.startswith(("crates/tui/src/", "crates/acp/src/"))
    ]
    schema = [
        p
        for p, _, _ in rows
        if p.startswith("crates/types/") or p.endswith("baselines/schemas.lock")
    ]

    files_map = "\n".join(
        f"- `{name}` ({stat[0]} file{'' if stat[0] == 1 else 's'}, "
        f"+{stat[1]}/-{stat[2]}) — <!-- why this area was touched -->"
        for name, stat in sorted(by_area.items())
    )
    src_table = (
        "| area | added | removed | net |\n|---|---|---|---|\n"
        + "\n".join(
            f"| `{name}` | +{s[0]} | -{s[1]} | {s[0] - s[1]:+d} |"
            for name, s in sorted(src_by_area.items())
        )
        if src_by_area
        else "No `crates/*/src` file changed."
    )

    out = [
        f"<!-- prefilled by `just pr-body` against {ref} ({base[:8]}); prose is yours -->\n",
        section(
            "Why needed",
            "<!-- the ask: Closes/Refs #N, the owner's words or the issue line quoted, "
            "and what stays broken if this does not land -->",
        ),
        section("Summary", "<!-- what changed and why, for a reader without the diff open -->"),
        section("User outcomes", "<!-- what a yi user can now do, see, or rely on -->"),
        section(
            "Seen red",
            "<!-- one line per new or changed test: test name · the failure it "
            "produced against the unfixed code · where the fixture came from. "
            "\"No tests changed\" if none. -->",
        ),
        section(
            "UI changes",
            "\n".join(f"- `{p}`" for p in ui) + "\n\n<!-- describe what moved, with the frames that show it -->"
            if ui
            else "None — no `crates/tui` or `crates/acp` file changed.",
        ),
        section("Files edited", files_map),
        section(
            "Schema changes",
            "\n".join(f"- `{p}`" for p in schema) + "\n\n<!-- fixtures added, never edited -->"
            if schema
            else "None.",
        ),
        section(
            "Deleted / alternatives",
            f"Net src LOC (`crates/*/src/**/*.rs`) vs the merge base with {ref}: "
            f"**{src_net:+d}** — this branch alone, and the table sums to it.\n\n"
            + src_table + "\n\n"
            + growth_verdict()
            + "\n\n<!-- what this removes, and the simpler options weighed; past the free band, "
            "what was weighed for deletion and why these bytes earn their place -->",
        ),
        section("Risk and rollback", "<!-- what could regress, how you would notice, how to revert -->"),
        section("Performance", "<!-- the hot path touched and a measured number, or \"No hot path touched\" and why -->"),
        section(
            "Architecture notes",
            "<!-- the change file under docs/changes/ (issue, growth, raises, decisions), feature-ledger rows (each names its journey test) -->",
        ),
    ]
    added, changed = surface_delta(base)
    out += surface_skeletons(added, changed)
    print("\n".join(out).rstrip())
    return 0


if __name__ == "__main__":
    sys.exit(main())
