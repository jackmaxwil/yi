#!/usr/bin/env python3
"""The har* skills under .ruler/skills are a copy of a canonical source that lives outside this
repo (030-style: they are normative for every Rust line), so nothing here can diff them against
it. This locks their hashes instead, making a hand-edit of the copy loud: the twelve sat 763
lines behind the source for two months while the copy agents loaded was current. Sync with
`just sync-skills <path-to-har-skills/skills>` (SRC is positional, so an `SRC=` prefix is read
as part of the path), which copies and then runs --update."""
import hashlib, json, pathlib, sys
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, BASE, fail

LOCK = BASE / "skills.json"
SKILLS = ROOT / ".ruler/skills"


def digest(text):
    return hashlib.sha256(text.encode()).hexdigest()


def current():
    return {
        str(path.relative_to(SKILLS)): digest(path.read_text())
        for path in sorted(SKILLS.glob("har*/SKILL.md"))
    }


def errors(now, locked, off=""):
    errs = []
    if off != "removed":
        errs += [f"{name} is locked but gone; a skill goes with its gate, never silently"
                 for name in sorted(set(locked) - set(now))]
    if off != "unlocked":
        errs += [f"{name} is not locked; run `just sync-skills` so the copy and the lock move together"
                 for name in sorted(set(now) - set(locked))]
    if off != "changed":
        errs += [f"{name} was edited in place; the canonical copy is outside this repo, so edit there "
                 f"and `just sync-skills`" for name in sorted(set(now) & set(locked)) if now[name] != locked[name]]
    return errs


def selfcheck():
    one, two = "har/SKILL.md", "har-io/SKILL.md"
    cases = {
        "removed": ({}, {one: "a"}),
        "unlocked": ({one: "a"}, {}),
        "changed": ({one: "a", two: "b"}, {one: "z", two: "b"}),
    }
    errs = []
    for name, (now, locked) in cases.items():
        if not errors(now, locked):
            errs.append(f"the {name} case produces no error, so that check is untested")
        if errors(now, locked, off=name):
            errs.append(f"selfcheck passes with the {name} check disabled, so it refutes nothing")
    if errors({one: "a"}, {one: "a"}):
        errs.append("an in-sync tree reports an error, so the gate is noise")
    fail(errs, "skills_sync --selfcheck")


def main():
    if "--selfcheck" in sys.argv:
        selfcheck()
        return
    now = current()
    if "--update" in sys.argv:
        LOCK.write_text(json.dumps({"files": now}, indent=2, sort_keys=True) + "\n")
        print(f"ok   skills_sync (baseline updated: {len(now)} skills)")
        return
    locked = json.loads(LOCK.read_text() or "{}").get("files", {})
    fail(errors(now, locked), f"skills_sync ({len(now)} skills)")


main()
