#!/usr/bin/env python3
"""What a real Yi-on-Yi session reads before its first word, in a lane and with --here.

Clones the repo into an empty scratch directory and copies in what a clone omits: the
checkout's untracked CLAUDE.md and its gitignored skill directories (.claude/skills and the
like, which a --here run lists as project skills). Then it runs the real binary on the faux
model twice under a scratch HOME and the caller's PATH (a plugin found there attaches its
card, as in a real run): once claiming a lane, once with --here. Each session's opening
ext_state entry is the prompt the extensions assembled; this prints what was copied, then its
bytes per slot and the instruction sources attached, with their trust. Zero spend, no key.
A CLAUDE.md byte-equal to AGENTS.md once comments are stripped is deduped and never shows.

    python3 evals/prefix_capture.py [--binary target/debug/yi] [--checkout DIR]
                                    [--claude PATH] [--scratch DIR]
"""

import argparse
import json
import os
import pathlib
import shutil
import subprocess
import sys
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[1]
SKILL_DIRS = (".yi", ".agents", ".pi", ".claude")


def opening(path):
    """The first ext_state of a session file: (slot, bytes) and (source, trust, bytes)."""
    for line in pathlib.Path(path).read_text().splitlines():
        entry = json.loads(line)
        if entry.get("customType") != "ext_state":
            continue
        data = entry.get("data") or {}
        slots = [(f"{slot['rank']}/{slot['name']}", len(slot["text"].encode()))
                 for slot in data.get("slots", [])]
        sources = [(item["source"], item.get("trust", ""), len(item["text"].encode()))
                   for item in data.get("yard", [])]
        return {"slots": slots, "sources": sources}
    raise ValueError(f"{path}: no ext_state entry")


def run(binary, cwd, home, sessions, here):
    argv = [str(binary), "ask", "--model", "faux/faux-1", "--session-dir", str(sessions)]
    argv += ["--here"] if here else []
    env = {"HOME": str(home), "PATH": os.environ.get("PATH", "/usr/bin:/bin")}
    done = subprocess.run(argv + ["hi"], cwd=cwd, env=env, capture_output=True, text=True,
                          timeout=300)
    if done.returncode != 0:
        raise RuntimeError(f"{' '.join(argv)} exited {done.returncode}: {done.stderr}")
    found = sorted(sessions.rglob("*.jsonl"))
    if len(found) != 1:
        raise RuntimeError(f"{sessions}: expected one session file, found {len(found)}")
    return opening(found[0])


def capture(binary, checkout, claude, scratch):
    repo = scratch / "repo"
    subprocess.run(["git", "clone", "-q", "--shared", str(ROOT), str(repo)], check=True)
    if claude.is_file():
        shutil.copy(claude, repo / "CLAUDE.md")
        print(f"copied CLAUDE.md from {claude} ({claude.stat().st_size} B)")
    else:
        print(f"no CLAUDE.md copied: {claude} is absent")
    for name in SKILL_DIRS:
        source, dest = checkout / name / "skills", repo / name / "skills"
        if source.is_dir() and not dest.exists():
            shutil.copytree(source, dest)
            print(f"copied {name}/skills from {checkout} ({len(list(source.iterdir()))} entries)")
    home = scratch / "home"
    home.mkdir()
    return {arm: run(binary, repo, home, scratch / f"sessions-{arm}", arm == "here")
            for arm in ("lane", "here")}


def report(arms):
    for arm, prefix in arms.items():
        total = sum(n for _, n in prefix["slots"]) + sum(n for *_, n in prefix["sources"])
        print(f"{arm}: {total} B")
        for slot, n in prefix["slots"]:
            print(f"  slot {slot} {n}")
        for source, trust, n in prefix["sources"]:
            print(f"  source {source} ({trust}) {n}")


def main(argv):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--binary", type=pathlib.Path, default=ROOT / "target/debug/yi")
    parser.add_argument("--checkout", type=pathlib.Path, default=ROOT,
                        help="whose untracked CLAUDE.md and skill directories are copied")
    parser.add_argument("--claude", type=pathlib.Path,
                        help="the CLAUDE.md to copy (default: the checkout's)")
    parser.add_argument("--scratch", type=pathlib.Path)
    args = parser.parse_args(argv)
    scratch = (args.scratch or pathlib.Path(tempfile.mkdtemp(prefix="yi-prefix-"))).resolve()
    scratch.mkdir(parents=True, exist_ok=True)
    if any(scratch.iterdir()):
        sys.exit(f"{scratch} is not empty: the checkpointer adds every file under the cwd")
    checkout = args.checkout.resolve()
    claude = args.claude or checkout / "CLAUDE.md"
    report(capture(args.binary.resolve(), checkout, claude, scratch))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
