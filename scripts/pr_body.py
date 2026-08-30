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
    if not GROWTH.exists():
        return f"growth gate: not present in this tree ({GROWTH.relative_to(ROOT)})"
    run = subprocess.run(
        [sys.executable, str(GROWTH)], cwd=ROOT, capture_output=True, text=True, check=False
    )
    text = (run.stdout + run.stderr).strip() or "(no output)"
    return "growth gate:\n\n```\n" + text + "\n```"


def section(title, body):
    return f"## {title}\n\n{body}\n"


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

    by_area = defaultdict(lambda: [0, 0, 0])
    src_net = 0
    for path, added, removed in rows:
        stat = by_area[area(path)]
        stat[0] += 1
        stat[1] += added
        stat[2] += removed
        if is_src(path):
            src_net += added - removed

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
    loc_table = "\n".join(
        f"| `{name}` | +{stat[1]} | -{stat[2]} |"
        for name, stat in sorted(by_area.items())
    )

    out = [
        f"<!-- prefilled by `just pr-body` against {ref} ({base[:8]}); prose is yours -->\n",
        section("Summary", "<!-- what changed and why, for a reader without the diff open -->"),
        section("User outcomes", "<!-- what a yi user can now do, see, or rely on -->"),
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
            f"Net src LOC (`crates/*/src/**/*.rs`, vs {ref}): **{src_net:+d}**\n\n"
            "| area | added | removed |\n|---|---|---|\n" + loc_table + "\n\n"
            + growth_verdict()
            + "\n\n<!-- past the free band: what was weighed for deletion, and why these bytes earn their place -->",
        ),
        section(
            "Architecture notes",
            "<!-- version bump, changelog row, D-rows, feature-ledger rows (each names its journey test) -->",
        ),
        section("Screenshots", "<!-- TUI frames or rendered output; \"None\" if none -->"),
    ]
    print("\n".join(out).rstrip())
    return 0


if __name__ == "__main__":
    sys.exit(main())
