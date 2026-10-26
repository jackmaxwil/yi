import contextlib, json, os, pathlib, re, shutil, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
# Net src lines a version may add unpriced (040). Here rather than in check_growth
# because check_pr_metadata prices the same band on a PR's own diff, and two
# copies of a budget is two budgets.
FREE_BAND = 150
BLOB_CAP, BLOB_ALLOW = 512_000, "scripts/guardrails/baselines/blob_allowlist.txt"
BASE = ROOT / "scripts/guardrails/baselines"
# Cargo builds under CARGO_TARGET_DIR when it is set; a fixed target/ path measured a stale binary.
DIST_BIN = ROOT / os.environ.get("CARGO_TARGET_DIR", "target") / "dist/yi"

SRC = re.compile(r"^crates/[^/]+/src/.+\.rs$")
TESTS = re.compile(r"^crates/[^/]+/tests/.+\.rs$")

def src_files():
    return sorted((ROOT / "crates").glob("*/src/**/*.rs"))

def prod_lines(path):
    return code_lines(path.read_text())

def code_lines(text, off=""):
    """The file's lines with every #[cfg(test)] item blanked, so a line number still names the file's line."""
    return [("" if test else line) for line, test in zip(text.splitlines(), test_mask(text, off))]

RAW = re.compile(r'b?r(#*)"')
CHAR = re.compile(r"b?'(?:\\.|[^\\'])'")
STRING = re.compile(r'(?:\\.|[^"\\])*"', re.S)

def normal_code(text, off=""):
    chars, at = list(text), 0
    while at < len(text):
        ch, word = text[at], at > 0 and (text[at - 1].isalnum() or text[at - 1] == "_")
        end = None
        if text.startswith("//", at):
            end = text.find("\n", at)
            end = len(text) if end < 0 else end
        elif off != "literals" and text.startswith("/*", at):
            close = text.find("*/", at + 2)
            end = len(text) if close < 0 else close + 2
        elif off != "literals" and not word and (raw := RAW.match(text, at)):
            close = text.find('"' + raw.group(1), raw.end())
            end = len(text) if close < 0 else close + 1 + len(raw.group(1))
        elif ch == '"':
            close = STRING.match(text, at + 1)
            end = len(text) if close is None else close.end()
        elif off != "chars" and not word and ch in "b'" and (lit := CHAR.match(text, at)):
            end = lit.end()
        if end is None:
            at += 1
            continue
        for index in range(at, end):
            if chars[index] != "\n":
                chars[index] = " "
        at = end
    return "".join(chars)

def test_mask(text, off=""):
    """Per line, whether a #[cfg(test)] attribute covers it: the attribute covers the one item after
    it, to its matching brace or its `;`. Incident: stopping at the first marker hid 2,285 production
    lines below mid-file test helpers from every gate that reads production code."""
    mask, done = [False] * len(text.splitlines()), 0
    visible = text if off == "markers" else normal_code(text, off)
    for marker in re.finditer(r"^[ \t]*#\[cfg\(test\)\]", visible, re.M):
        if marker.start() < done:
            continue
        done = len(text) if off == "first" else item_end(text, marker.end())
        first, last = text.count("\n", 0, marker.start()), text.count("\n", 0, done)
        mask[first:last + 1] = [True] * (last + 1 - first)
    return mask

def item_end(text, at):
    """The offset of the `}` or `;` that ends the item starting at `at`, read past strings, raw
    strings, char literals and comments, so a brace in a test's JSON fixture counts for nothing."""
    # ponytail: block comments do not nest here; none in the tree sits inside a test item.
    depth, opened = 0, False
    while 0 <= at < len(text):
        ch, word = text[at], at > 0 and (text[at - 1].isalnum() or text[at - 1] == "_")
        if text.startswith("//", at):
            at = text.find("\n", at)
        elif text.startswith("/*", at):
            at = text.find("*/", at) + 1 or -1
        elif not word and (raw := RAW.match(text, at)):
            at = text.find('"' + raw.group(1), raw.end())
            at = at + len(raw.group(1)) if at >= 0 else -1
        elif ch == '"':
            close = STRING.match(text, at + 1)
            at = close.end() - 1 if close else -1
        elif not word and ch in "b'" and (lit := CHAR.match(text, at)):
            at = lit.end() - 1
        elif ch in "{([":
            depth, opened = depth + 1, opened or ch == "{"
        elif ch in "})]":
            depth -= 1
            if depth <= 0 and opened:
                return at
        elif ch == ";" and depth == 0:
            return at
        at = at + 1 if at >= 0 else -1
    return len(text)

def git(*args):
    return subprocess.run(("git", "-C", str(ROOT)) + args, capture_output=True, text=True, check=False)

def base_commit():
    """(ref, sha) where this branch left main: the size gates price growth from here, so a shrink
    on main tightens the next branch with no commit, and two branches never write one ceiling."""
    # Mid-merge the commit being made has MERGE_HEAD as a parent too: its fork, not HEAD's, is judged.
    tips = ("HEAD", "MERGE_HEAD") if git("rev-parse", "--verify", "--quiet", "MERGE_HEAD").returncode == 0 else ("HEAD",)
    for ref in ("origin/main", "main"):
        if git("rev-parse", "--verify", "--quiet", ref).returncode == 0:
            found = git("merge-base", ref, *tips)
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

@contextlib.contextmanager
def scratch_repo(prefix, seed):
    """A git repo holding these guardrails and `seed` ({path: text or bytes}) on main, checked out on
    `topic`; yields (dir, run), run being a command in it with CARGO_TARGET_DIR unset; removed after."""
    tmp = pathlib.Path(tempfile.mkdtemp(prefix=prefix))
    env = {k: v for k, v in os.environ.items() if k != "CARGO_TARGET_DIR"}
    run = lambda *a: subprocess.run(a, cwd=tmp, capture_output=True, text=True, env=env)
    try:
        shutil.copytree(pathlib.Path(__file__).parent, tmp / "scripts/guardrails", ignore=shutil.ignore_patterns("baselines", "__pycache__"))
        for name, body in seed.items():
            (tmp / name).parent.mkdir(parents=True, exist_ok=True)
            (tmp / name).write_bytes(body if isinstance(body, bytes) else body.encode())
        for args in (("init", "-q", "-b", "main"), ("add", "-A"), ("commit", "-q", "-m", "seed"), ("checkout", "-q", "-b", "topic")):
            run("git", "-c", "user.name=t", "-c", "user.email=t@t", *args)
        yield tmp, run
    finally:
        shutil.rmtree(tmp)

def no_fork(name):
    fail(["no origin/main or main to find this branch's fork point; a shallow clone cannot run this gate"], name)

def fail(msgs, name):
    if msgs:
        print(f"FAIL {name}")
        for m in msgs:
            print(f"  {m}")
        sys.exit(1)
    print(f"ok   {name}")
