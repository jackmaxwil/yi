#!/usr/bin/env python3
"""Refuse a fixer's push that reverses what the PR's author did on purpose: a commit the push adds may
not put back a line an earlier commit of the PR deleted, nor delete five or more substantive lines one
added. Nothing exempts a fix, a review finding naming the file included: the hold shows the finding
beside the hit, so the owner reads reviewer against author.

Incident: on 2026-11-06 the Kimi land-fix put back the README install block the owner had cut and cut
their architecture chart (#2062 at 7b3e430e9), and cut #1829's upstream pin (30249812c); the merge queue
landed both. A review finding had asked for each reversal, so an exemption for a file a finding names
would have let both through. The fixer is named by range, not
by author: land-fix commits under the owner's name, and pr_autofix's commits are every one it pushes.

    fixer_guard.py --pr N --from <head before the fix> [--to HEAD] [--base origin/main]

exits 1 on a hit, after labelling the PR `autofix:hold` and commenting each hit; land-fix calls it
before its push. pr_autofix calls `scan` on the commits it is about to push.
"""
import argparse
import pathlib
import re
import subprocess
import sys
from collections import defaultdict

ROOT = pathlib.Path(__file__).resolve().parents[1]
CUT_AT = 5
# Measured numbers are the gates' to move: a Ratchet puts an old value back with no author's intent in it.
MEASURED = ("scripts/guardrails/baselines/",)
MARK = "<!-- yi-fixer-guard -->"
REFUSED = ("**Fixer refused**: its push would undo what this PR's own commits did on purpose, so nothing was pushed "
           "and `autofix:hold` is set. A review finding that asked for it is listed under its hit and does not exempt "
           "it. The owner decides: push the fix by hand, or remove the label for the fixer to try again.")
FIXTURES = ROOT / "scripts/guardrails/fixtures/fixer_guard"
# Each fixture is `git log --reverse --no-merges -p --unified=0 --format='commit %H'` over the merged
# PR's commits, limited to the files its fixer commit touched.
INCIDENTS = {
    "pr2062.log": ("7b3e430e9385fd8d243ba53b91423d32404d02ec",
                   {("resurrect", "README.md", 13), ("cut", "README.md", 58)}),
    "pr1829.log": ("30249812ce529c084c4f85c02dc34c13302e8800",
                   {("resurrect", "crates/ai/src/openai.rs", 1), ("resurrect", "crates/ai/src/request.rs", 1),
                    ("cut", "crates/ai/src/breakpoints.rs", 7), ("cut", "crates/ai/src/request.rs", 20),
                    ("cut", "crates/ai/tests/openrouter.rs", 12), ("cut", "crates/runtime/src/cache_miss.rs", 23),
                    ("cut", "crates/runtime/src/provider.rs", 6), ("cut", "crates/runtime/tests/cache_miss.rs", 16),
                    ("cut", "docs/changes/2026-10-29-cache-marks-from-the-ledger.md", 6)}),
}


class Reversal(RuntimeError):
    def __init__(self, hits):
        super().__init__(f"the fix reverses the author's own change in {len(hits)} place(s)")
        self.hits = hits


def substantive(line):
    """A blank, a brace or a short comment comes and goes in any edit; a line of content does not."""
    s = line.strip()
    return len(s) >= 40 or (len(s) >= 25 and not s.startswith(("//", "#")))


def commits(log):
    """(sha, {path: (added, deleted)}) per commit. A deleted `-- x` line prints as `--- x`, so file
    headers are read only between `diff --git` and the first hunk."""
    out, files, header, old, path = [], None, False, None, None
    for line in log.splitlines():
        if line.startswith("commit "):
            files, header, path = defaultdict(lambda: ([], [])), False, None
            out.append((line.split()[1], files))
        elif line.startswith("diff --git "):
            header, old, path = True, None, None
        elif header and line.startswith("--- "):
            old = line[6:] if line.startswith("--- a/") else None
        elif header and line.startswith("+++ "):
            path = line[6:] if line.startswith("+++ b/") else old
        elif line.startswith("@@"):
            header = False
        elif not header and path and files is not None and line[:1] in ("+", "-"):
            files[path][line[0] == "-"].append(line[1:])
    return out


def hits(log, fixer, off=""):
    """(kind, sha, path, lines, by) per reversal in a commit of `fixer`; `off` disables one check."""
    deleted, added, out = defaultdict(dict), defaultdict(dict), []
    for sha, files in commits(log):
        for path, (adds, dels) in files.items() if sha in fixer else ():
            if path.startswith(MEASURED):
                continue
            back = [line for line in adds if substantive(line) and line in deleted[path]]
            if back and off != "resurrect":
                out.append(("resurrect", sha, path, back, deleted[path][back[0]]))
            # A line deleted here and added back in the same file is a move, not a cut.
            cut = [line for line in dels if substantive(line) and line in added[path] and (off == "move" or line not in adds)]
            if len(cut) >= (1 if off == "threshold" else CUT_AT) and off != "cut":
                out.append(("cut", sha, path, cut, added[path][cut[0]]))
        for path, (adds, dels) in files.items():
            for line in dels:
                if line not in added[path]:
                    deleted[path].setdefault(line, sha)
            for line in adds:
                added[path].setdefault(line, sha)
    return out


def git(repo, *args):
    return subprocess.run(["git", "-C", str(repo), "-c", "core.quotePath=false", *args],
                          capture_output=True, text=True, check=True).stdout


def scan(repo, base, before, after):
    """The reversals in the commits `before..after` adds, read against the PR's history since `base`."""
    # The base is excluded too: a merge of main brings main's commits into `before..after`.
    fixer = set(git(repo, "rev-list", "--no-merges", after, "--not", before, base).split())
    if not fixer:
        return []
    log = git(repo, "log", "--reverse", "--no-merges", "--no-color", "--no-ext-diff", "-p", "--unified=0",
              "--format=commit %H", f"{base}..{after}")
    return hits(log, fixer)


def report(found, findings=()):
    """Each hit, and under it every review finding that names its file: the reviewer may be the one who asked."""
    rows = []
    for kind, sha, path, lines, by in found:
        verb = "re-adds" if kind == "resurrect" else "deletes"
        whose = "deleted" if kind == "resurrect" else "added"
        rows.append(f"- `{sha[:9]}` {verb} {len(lines)} line(s) of `{path}` that `{by[:9]}` {whose}; "
                    f"first: `{lines[0].strip()[:100].replace('`', '')}`")
        rows += [f"  - asked by a {f.get('severity')} finding at `{path}:{f.get('line')}`: {(f.get('claim') or '')[:200]}"
                 for f in findings if f.get("path") == path]
    return "\n".join(rows)


def comment(found, findings=()):
    return f"{MARK}\n{REFUSED}\n\n{report(found, findings)}\n"


def selfcheck():
    errs = []
    five = [f"a line of real content, number {n}" for n in range(5)]
    patch = lambda sha, minus, plus: (f"commit {sha}\ndiff --git a/f b/f\n--- a/f\n+++ b/f\n@@ -1 +1 @@\n"
                                      + "".join(f"-{line}\n" for line in minus) + "".join(f"+{line}\n" for line in plus))
    for off in ("", "resurrect", "cut", "threshold", "move"):
        bad = []
        for name, (fixer, want) in INCIDENTS.items():
            log = (FIXTURES / name).read_text()
            got = {(kind, path, len(lines)) for kind, _, path, lines, _ in hits(log, {fixer}, off=off)}
            if got != want:
                bad.append(f"{name}: hits {sorted(got ^ want)} differ from the incident's")
            authors = {sha for sha, _ in commits(log)} - {fixer}
            if hits(log, authors, off=off):
                bad.append(f"{name}: the author's own commits are read as a fixer's")
        if hits(patch("a", [], five) + patch("b", five, five[::-1]), {"b"}, off=off):
            bad.append("a move inside one file is read as a cut")
        asked = {"path": "f", "line": 1, "severity": "high", "claim": "restore what the PR cut"}
        if "asked by a high finding at `f:1`" not in report(hits(patch("a", five, []) + patch("b", [], five), {"b"}), [asked]):
            bad.append("the hold does not name the finding that asked for the reversal")
        if off and not bad:
            errs.append(f"selfcheck passes with the {off} check disabled, so it refutes nothing")
        if not off:
            errs += bad
    return errs


def main(argv):
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--selfcheck", action="store_true")
    ap.add_argument("--pr", type=int)
    ap.add_argument("--from", dest="before")
    ap.add_argument("--to", dest="after", default="HEAD")
    ap.add_argument("--base", default="origin/main")
    args = ap.parse_args(argv)
    if args.selfcheck:
        errs = selfcheck()
        print("\n".join(f"FAIL fixer_guard selfcheck: {e}" for e in errs) or "ok   fixer_guard selfcheck")
        return 1 if errs else 0
    if not (args.pr and args.before):
        ap.error("--pr and --from are required")
    sys.path[:0] = [str(ROOT / "scripts"), str(ROOT / "scripts/guardrails")]
    import forge_pr, pr_queue, pr_review  # noqa: E401
    repo = forge_pr.repo()
    found = scan(pathlib.Path.cwd(), args.base, args.before, args.after)
    if not found:
        print(f"#{args.pr}: the fix reverses nothing the author did")
        return 0
    pr_queue.set_label(args.pr, "autofix:hold", True)
    rounds = pr_review.rounds_of(pr_review.comments(repo, args.pr), pr_review.authors(), args.pr)
    findings = [f for r in rounds for f in r["findings"]]
    pr_queue.api("POST", f"issues/{args.pr}/comments", {"body": comment(found, findings)})
    print(f"#{args.pr}: refused, held and commented:\n{report(found, findings)}")
    return 1


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
