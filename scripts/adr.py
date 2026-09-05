#!/usr/bin/env python3
"""One ADR per decision-log row, written from the row rather than beside it.

The row in docs/ARCHITECTURE.md is the record; an ADR that restates it by hand drifts
from it, and four landings in a row shipped no ADR at all because writing one was a
separate act of prose. This renders the row's three cells and files the index line.
"""
import pathlib, re, sys, textwrap

ROOT = pathlib.Path(__file__).resolve().parents[1]
ARCH = ROOT / "docs/ARCHITECTURE.md"
INDEX = ROOT / "docs/solutions/README.md"
ADRS = ROOT / "docs/solutions/adr"
# The index lists an ADR per line; the last one is where the next line goes.
INDEX_LINE = re.compile(r"^- \[D(\d+)\]\(adr/d\1\.md\)")


def row(number):
    for line in ARCH.read_text().splitlines():
        if line.startswith(f"| D{number} |"):
            cells = [cell.strip() for cell in line.strip().strip("|").split(" | ")]
            if len(cells) == 4:
                return cells[1], cells[2], cells[3]
            return None
    return None


def title_of(decision):
    """The row opens with the claim and then qualifies it after a colon; the claim titles."""
    head = decision.split(":")[0].strip().rstrip(",")
    return head if len(head) <= 80 else head[:77].rsplit(" ", 1)[0] + "..."


def body(cell):
    """Wrapped at the width the hand-written ADRs already use, so a diff reads by clause."""
    return textwrap.fill(cell[:1].upper() + cell[1:], width=84, break_long_words=False)


def render(number, decision, why, reversible):
    return (
        f"# D{number}: {title_of(decision)}\n\nStatus: accepted\n\n"
        f"## Decision\n\n{body(decision)}\n\n## Why\n\n{body(why)}\n\n"
        f"## Reversible via\n\n{body(reversible)}\n"
    )


def index_lines(text, number, title):
    lines = text.splitlines()
    last = max(i for i, line in enumerate(lines) if INDEX_LINE.match(line))
    entry = f"- [D{number}](adr/d{number}.md) - {title}"
    if entry in lines:
        return None
    return "\n".join(lines[: last + 1] + [entry] + lines[last + 1 :]) + "\n"


def main(argv):
    if not argv or not argv[0].lstrip("Dd").isdigit():
        print("usage: just adr <D-number>")
        return 1
    number = argv[0].lstrip("Dd")
    cells = row(number)
    if cells is None:
        print(f"adr: no D{number} row in docs/ARCHITECTURE.md — write the row first")
        return 1
    path = ADRS / f"d{number}.md"
    path.write_text(render(number, *cells))
    updated = index_lines(INDEX.read_text(), number, title_of(cells[0]))
    if updated is not None:
        INDEX.write_text(updated)
    print(f"adr: {path.relative_to(ROOT)} + index line")
    return 0


def selfcheck():
    assert title_of("git is the registry of lanes: `git worktree list` holds the slots") == "git is the registry of lanes"
    assert title_of("landing state is an event") == "landing state is an event"
    out = render("9", "a is b: and more", "because", "delete it")
    assert out.startswith("# D9: a is b\n") and out.endswith("Delete it\n"), out
    assert "## Why\n\nBecause\n" in out
    assert max(len(line) for line in body("word " * 60).splitlines()) <= 84
    text = "- [D1](adr/d1.md) - one\n- [D2](adr/d2.md) - two\n\nD3 has no ADR.\n"
    got = index_lines(text, "4", "four")
    assert got.splitlines()[2] == "- [D4](adr/d4.md) - four", got
    assert index_lines(got, "4", "four") is None
    print("ok   adr selfcheck")


if __name__ == "__main__":
    if "--selfcheck" in sys.argv:
        selfcheck()
        sys.exit(0)
    sys.exit(main(sys.argv[1:]))
