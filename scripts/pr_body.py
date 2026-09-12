#!/usr/bin/env python3
"""Prefill the PR narrative's mechanical halves from the diff against main.

The prose sections stay the author's; only the counted ones are filled, because
a hand-typed net-LOC line is an estimate and the growth budget prices the
measured number. Prints to stdout — piping is the caller's business.
"""
import pathlib
import subprocess
import sys
from collections import defaultdict

ROOT = pathlib.Path(__file__).resolve().parents[1]
GROWTH = ROOT / "scripts/guardrails/check_growth.py"


def git(*args):
    return subprocess.run(
        ("git", "-C", str(ROOT)) + args, capture_output=True, text=True, check=False
    )


def base_commit():
    for ref in ("origin/main", "main"):
        if git("rev-parse", "--verify", "--quiet", ref).returncode == 0:
            found = git("merge-base", ref, "HEAD")
            if found.returncode == 0:
                return ref, found.stdout.strip()
    return None, None


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
    """The gate answers a different question from the headline: it measures the whole
    tree against the last `--update`, so everything landed since that version counts,
    not this branch alone. Two numbers under one heading needs both named."""
    if not GROWTH.exists():
        return f"growth gate: not present in this tree ({GROWTH.relative_to(ROOT)})"
    run = subprocess.run(
        [sys.executable, str(GROWTH)], cwd=ROOT, capture_output=True, text=True, check=False
    )
    text = (run.stdout + run.stderr).strip() or "(no output)"
    return (
        "growth gate — the whole tree against the growth baseline "
        "(`baselines/src_loc.json`, the version its last `--update` measured), so it "
        "counts every branch landed since, not this one alone:\n\n```\n" + text + "\n```"
    )


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


def surface_diff(was, now):
    """(added, changed) lock keys between two surfaces. A removal makes no new
    claim and asks nothing, so removed keys are not reported."""
    added = sorted(key for key in now if key not in was)
    changed = sorted(key for key in now if key in was and was[key] != now[key])
    return added, changed


def surface_delta(base):
    """(added, changed) keys in the tool-surface lock between the merge base and
    the working tree. A lock missing at base is being seeded and asks nothing."""
    import json

    before = git("show", f"{base}:{SURFACE_LOCK}")
    if before.returncode != 0:
        return [], []
    now_path = ROOT / SURFACE_LOCK
    if not now_path.exists():
        raise SystemExit(LOCK_MISSING)
    return surface_diff(json.loads(before.stdout), json.loads(now_path.read_text()))


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
    assert surface_diff({"tool:read": "a"}, {"tool:read": "a"}) == ([], [])
    assert surface_diff({"tool:read": "a"}, {"tool:read": "a", "tool:foo": "b"}) == (["tool:foo"], [])
    assert surface_diff({"tool:read": "a"}, {"tool:read": "z"}) == ([], ["tool:read"])
    assert surface_diff({"tool:read": "a", "tool:gone": "b"}, {"tool:read": "a"}) == ([], [])
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
            "\n".join(f"- `{p}`" for p in ui) + "\n\n<!-- describe what moved -->"
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
            "LOC and justification",
            f"Net src LOC (`crates/*/src/**/*.rs`) vs the merge base with {ref}: "
            f"**{src_net:+d}** — this branch alone, and the table sums to it.\n\n"
            + src_table + "\n\n"
            + growth_verdict()
            + "\n\n<!-- past the free band: what was weighed for deletion, and why these bytes earn their place -->",
        ),
        section(
            "Architecture notes",
            "<!-- version bump, changelog row, D-rows, feature-ledger rows (each names its journey test) -->",
        ),
    ]
    added, changed = surface_delta(base)
    out += surface_skeletons(added, changed)
    out.append(section("Screenshots", "<!-- TUI frames or rendered output; \"None\" if none -->"))
    print("\n".join(out).rstrip())
    return 0


if __name__ == "__main__":
    sys.exit(main())
