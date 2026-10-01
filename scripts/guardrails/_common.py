import json, os, pathlib, re, subprocess, sys

ROOT = pathlib.Path(__file__).resolve().parents[2]
# Net src lines a version may add unpriced (040). Here rather than in check_growth
# because check_pr_metadata prices the same band on a PR's own diff, and two
# copies of a budget is two budgets.
FREE_BAND = 150
BASE = ROOT / "scripts/guardrails/baselines"
# Cargo builds under CARGO_TARGET_DIR when it is set; a fixed target/ path measured a stale binary.
DIST_BIN = ROOT / os.environ.get("CARGO_TARGET_DIR", "target") / "dist/yi"

SRC = re.compile(r"^crates/[^/]+/src/.+\.rs$")
TESTS = re.compile(r"^crates/[^/]+/tests/.+\.rs$")

def src_files():
    return sorted((ROOT / "crates").glob("*/src/**/*.rs"))

def prod_lines(path):
    return code_lines(path.read_text())

def code_lines(text):
    """Lines before a #[cfg(test)] marker; inline test mods sit at file bottom by convention."""
    out = []
    for line in text.splitlines():
        if line.strip().startswith("#[cfg(test)]"):
            break
        out.append(line)
    return out

def git(*args):
    return subprocess.run(("git", "-C", str(ROOT)) + args, capture_output=True, text=True, check=False)

def base_commit():
    """(ref, sha) where this branch left main: the size gates price growth from here, so a shrink
    on main tightens the next branch with no commit, and two branches never write one ceiling."""
    for ref in ("origin/main", "main"):
        if git("rev-parse", "--verify", "--quiet", ref).returncode == 0:
            found = git("merge-base", ref, "HEAD")
            if found.returncode == 0:
                return ref, found.stdout.strip()
    return None, None

def fork():
    return base_commit()[1]

def texts(pattern, rev=None):
    """{repo path: text} for files under crates/ matching `pattern`, in the working tree or at `rev`."""
    if rev is None:
        return {str(p.relative_to(ROOT)): p.read_text(errors="replace") for p in sorted((ROOT / "crates").rglob("*.rs"))
                if pattern.match(str(p.relative_to(ROOT)))}
    names = [n for n in git("ls-tree", "-r", "--name-only", rev, "--", "crates").stdout.splitlines() if pattern.match(n)]
    batch = subprocess.run(("git", "-C", str(ROOT), "cat-file", "--batch"), input="".join(f"{rev}:{n}\n" for n in names).encode(),
                           capture_output=True, check=True).stdout
    out, at = {}, 0
    for name in names:
        head_end = batch.index(b"\n", at)
        header = batch[at:head_end].split()
        if len(header) != 3:
            fail([f"git cat-file has no blob for {rev[:8]}:{name} ({header[-1].decode()}); fetch the full history"], "fork texts")
        size = int(header[2])
        out[name] = batch[head_end + 1:head_end + 1 + size].decode(errors="replace")
        at = head_end + 1 + size + 1
    return out

def no_fork(name):
    fail(["no origin/main or main to find this branch's fork point; a shallow clone cannot run this gate"], name)

def fail(msgs, name):
    if msgs:
        print(f"FAIL {name}")
        for m in msgs:
            print(f"  {m}")
        sys.exit(1)
    print(f"ok   {name}")
