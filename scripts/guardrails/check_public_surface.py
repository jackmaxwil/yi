#!/usr/bin/env python3
"""The public mirror carries nothing of the estate (D172).

scripts/mirror/sync.sh publishes a derived history: the paths in exclude.txt dropped, the
literals in replace.txt substituted, identities through mailmap.txt. This gate holds the tree
to what that publishes: after the same exclusion and substitution, no tracked file matches a
pattern in deny.txt. A file that names the estate fails here until it is excluded, which also
drops it from the mirror, or its text is replaced: one list does both, so the gate cannot drift
from the mirror. `--stdin` holds the rewritten history sync.sh pipes in, a `git log -p` whose
commits each open on a \x1e line, to the same patterns with nothing substituted: every blob
line, message and identity the mirror would publish. A branch's own commit messages are held to
them here too, substituted as the mirror does.
Incident: two messages quoting a planted address stopped the mirror for days, because only that
post-merge run read messages. Identities go through mailmap.txt, which this does not model.

The paths .gitguardian.yaml ignores are the planted fakes and vendored upstream samples and are
not scanned. filter-repo substitutes nothing in a binary blob, so neither does this. The mirror
itself carries no rules, and there the gate announces a skip.
"""
import fnmatch, re, subprocess, sys, pathlib
sys.path.insert(0, str(pathlib.Path(__file__).parent))
from _common import ROOT, base_commit, fail, git, scratch_repo

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

def messages(log, deny, swaps):
    errs = []
    for record in filter(None, log.split("\x1e")):
        sha, _, body = record.partition("\n")
        errs += leaks(sha, body, deny, swaps)
    return errs

def branch_messages(deny, swaps):
    base = base_commit()[1]
    if base is None:
        return ["no origin/main or main to compare against, so this branch's messages went unread"]
    log = git("log", "--format=%x1e%h%n%B", "HEAD", "--not", base)
    if log.returncode != 0:
        return [f"git log failed: {log.stderr.strip()}"]
    return messages(log.stdout, deny, swaps)

def wired():
    seed = {"scripts/mirror/deny.txt": "(?i)estate\\.lan\n", "scripts/mirror/exclude.txt": "scripts/\n",
            "scripts/mirror/replace.txt": "git.estate.lan==>git.example.invalid\n", ".gitguardian.yaml": "paths-ignore: []\n"}
    gate = ("python3", "scripts/guardrails/check_public_surface.py")
    with scratch_repo("public-surface-", seed) as (_, run):
        commit = lambda m: run("git", "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", m)
        commit("Name the host\n\nserved from git.estate.lan")
        passed = run(*gate)
        assert passed.returncode == 0, f"a message the mirror rewrites was refused: {passed.stdout}"
        commit("Name the host\n\nserved from ESTATE.LAN")
        refused = run(*gate)
        assert refused.returncode == 1 and "ESTATE.LAN" in refused.stdout, f"a denied message passed: {refused.stdout}"

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
    # The planted fakes are skipped, at any depth, and no real source is: `tree()` and `history()`
    # match this list with fnmatch, so the selfcheck does too.
    nested = "crates/runtime/tests/fixtures/behavior/a/b/planted-key"
    assert any(fnmatch.fnmatch(nested, g) for g in unscanned()), "a nested planted path is not skipped"
    for path in (
        "crates/oauth/tests/device_login.rs",
        "crates/oauth/tests/flow.rs",
        "crates/runtime/tests/kept_rules.rs",
        "docs/archive/plans/2026-09-06-prompt-surface/prior-art-codex-jcode.md",
        "docs/archive/plans/2026-09-06-prompt-surface/openai.md",
    ):
        assert not any(fnmatch.fnmatch(path, g) for g in unscanned()), (
            f"public-surface scanning skips real source: {path}")
    # A message is judged as the mirror rewrites it, and a hit names its own commit.
    log = "\x1eaaa\nhost git.estate.lan\n\n\x1ebbb\nsubject\n\nsays ESTATE.LAN\n"
    assert messages(log, deny, swaps) == ["bbb:3: says ESTATE.LAN"]
    wired()
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
    errs = tree(deny, swaps, rule_lines(RULES / "exclude.txt")) + branch_messages(deny, swaps)
    hint = ["the mirror would publish these: drop the path in scripts/mirror/exclude.txt, reword",
            "a commit's message, or substitute the text in replace.txt (docs/FORGE.md, D172)"]
    fail(errs + hint if errs else [], "public_surface")

if __name__ == "__main__":
    main()
