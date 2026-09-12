#!/usr/bin/env python3
"""The public mirror carries nothing of the estate (D172).

scripts/mirror/sync.sh publishes a derived history: the paths in exclude.txt dropped, the
literals in replace.txt substituted, identities through mailmap.txt. This gate holds the tree
to what that publishes: after the same exclusion and substitution, no tracked file matches a
pattern in deny.txt. A file that names the estate fails here until it is excluded, which also
drops it from the mirror, or its text is replaced: one list does both, so the gate cannot drift
from the mirror. `--stdin` holds the rewritten history sync.sh pipes in, a `git log -p` whose
commits each open on a \x1e line, to the same patterns with nothing substituted: every blob
line, message and identity the mirror would publish.

The paths .gitguardian.yaml ignores are the planted fakes and vendored upstream samples and are
not scanned. filter-repo substitutes nothing in a binary blob, so neither does this. The mirror
itself carries no rules, and there the gate announces a skip.
"""
import fnmatch, re, subprocess, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, fail

RULES = ROOT / "scripts/mirror"

def rule_lines(path, comments=True):
    return [l for l in path.read_text().splitlines() if l and not (comments and l.startswith("#"))]

def swaps_of(lines):
    # filter-repo reads every replace.txt line, a `#` one included, and a line with no `==>`
    # replaces its text with ***REMOVED***. Its regex: and glob: forms are not modelled here.
    unmodelled = [f"replace.txt: {l!r} is not a plain literal"
                  for l in lines if l.startswith(("regex:", "glob:", "literal:"))]
    if unmodelled:
        fail(unmodelled, "public_surface")
    return [l.rsplit("==>", 1) if "==>" in l else [l, "***REMOVED***"] for l in lines]

def excluded(path, prefixes):
    return any(path == p.rstrip("/") or path.startswith(p.rstrip("/") + "/") for p in prefixes)

def leaks(name, text, deny, swaps):
    for old, new in swaps:
        text = text.replace(old, new)
    return [f"{name}:{n}: {line.strip()[:160]}"
            for n, line in enumerate(text.splitlines(), 1) if any(p.search(line) for p in deny)]

def unscanned():
    text = (ROOT / ".gitguardian.yaml").read_text()
    return re.findall(r'^\s*-\s*"([^"]+)"', text, re.M)

def history(lines, deny, skip):
    path, errs = "(message)", []
    for line in lines:
        if line.startswith("\x1e"):
            path = "(message)"
        elif line.startswith("diff --git "):
            path = line.rstrip("\n").rsplit(" b/", 1)[-1]
        if any(p.search(line) for p in deny) and not any(fnmatch.fnmatch(path, g) for g in skip):
            errs.append(f"{path}: {line.strip()[:160]}")
    return errs

def tree(deny, swaps, prefixes):
    skip = unscanned()
    out = subprocess.run(["git", "ls-files", "-z"], cwd=ROOT, capture_output=True,
                         check=True).stdout.decode().split("\0")
    errs = []
    for rel in filter(None, out):
        path = ROOT / rel
        if excluded(rel, prefixes) or any(fnmatch.fnmatch(rel, g) for g in skip):
            continue
        if path.is_symlink() or not path.is_file():
            continue
        data = path.read_bytes()
        binary = b"\0" in data[:8192]
        errs += leaks(rel, data.decode("utf-8", "replace"), deny, [] if binary else swaps)
    return errs

def selfcheck():
    deny = [re.compile(r"(?i)estate\.lan")]
    swaps = swaps_of(["git.estate.lan==>git.example.invalid", "gone"])
    # The deny patterns find a line, and name it.
    assert leaks("a", "x\nhost git.estate.lan\n", deny, []) == ["a:2: host git.estate.lan"]
    # A replaced literal is what the mirror carries, so it is not a leak...
    assert leaks("a", "host git.estate.lan", deny, swaps) == []
    # ...and a replacement hides only its own literal.
    assert leaks("a", "git.estate.lan and ESTATE.LAN", deny, swaps) != []
    # A bare line is replaced with filter-repo's marker, as filter-repo does.
    assert swaps[1] == ["gone", "***REMOVED***"]
    # An excluded path is itself and everything under it, never a string prefix.
    assert excluded("docs/x.md", ["docs/x.md"]) and excluded("s/m/a.txt", ["s/m/"])
    assert excluded("s/m/a.txt", ["s/m"]) and not excluded("s/mx/a.txt", ["s/m"])
    # In history a diff's path is skipped as a file is, and the next commit's message is not.
    log = ["\x1eme", "diff --git a/p/planted b/p/planted", "+estate.lan", "\x1eestate.lan"]
    assert history(log, deny, ["p/*"]) == ["(message): estate.lan"]
    assert history(log, deny, []) == ["p/planted: +estate.lan", "(message): estate.lan"]
    # The planted fakes are skipped by the list GitGuardian already reads.
    assert "**/fixtures/planted*" in unscanned()
    print("ok   public_surface --selfcheck")

def main():
    if "--selfcheck" in sys.argv:
        return selfcheck()
    if not (RULES / "deny.txt").is_file():
        return print("skip public_surface (no scripts/mirror: this tree is the mirror)")
    deny = [re.compile(p) for p in rule_lines(RULES / "deny.txt")]
    if "--stdin" in sys.argv:
        return fail(history(sys.stdin, deny, unscanned()), "public_surface --stdin")
    swaps = swaps_of(rule_lines(RULES / "replace.txt", comments=False))
    errs = tree(deny, swaps, rule_lines(RULES / "exclude.txt"))
    hint = ["the mirror would publish these: drop the path in scripts/mirror/exclude.txt,",
            "or substitute the text in replace.txt (docs/FORGE.md, D172)"]
    fail(errs + hint if errs else [], "public_surface")

main()
