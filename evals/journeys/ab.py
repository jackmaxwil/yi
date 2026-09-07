#!/usr/bin/env python3
"""Run the journey prompts against a real model under one prompt ref and score the
session files with the extractor. One JSONL per prompt lands under --out/<ref>/;
the signals table is the number a prompt change is judged by."""
import argparse, os, pathlib, shutil, subprocess, sys, tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
PROMPTS = pathlib.Path(__file__).resolve().parent / "prompts" / "prompts.txt"


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--model", required=True)
    parser.add_argument("--ref", required=True, help="a label for the prompt under test, e.g. the git ref")
    parser.add_argument("--binary", default=str(ROOT / "target/debug/yi"))
    parser.add_argument("--cwd", default=str(ROOT))
    parser.add_argument("--out", default=str(ROOT / ".yi/journeys"))
    parser.add_argument("--only", help="run only prompts whose class matches")
    parser.add_argument("--timeout", type=int, default=900)
    args = parser.parse_args(argv)
    out = pathlib.Path(args.out) / args.ref
    out.mkdir(parents=True, exist_ok=True)
    (pathlib.Path(args.out) / ".gitignore").write_text("*\n")
    home = tempfile.mkdtemp(prefix="yi-journey-home-")
    config = pathlib.Path(home) / ".yi" / "config.json"
    config.parent.mkdir(parents=True, exist_ok=True)
    config.write_text('{"telemetry":{"enabled":true}}')
    env = dict(os.environ, HOME=home)
    ran = 0
    for line in PROMPTS.read_text().splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        klass, prompt = line.split(" ", 1)
        if args.only and args.only != klass:
            continue
        ran += 1
        target = out / f"{ran:02}-{klass}.jsonl"
        with target.open("w") as sink:
            subprocess.run(
                [args.binary, "ask", "--here", "--model", args.model, "--json", prompt],
                cwd=args.cwd, env=env, stdout=sink, stderr=subprocess.STDOUT,
                timeout=args.timeout, check=False,
            )
        print(f"ran {klass}: {target.name} ({target.stat().st_size} bytes)", file=sys.stderr)
    sessions = pathlib.Path(home) / ".yi" / "sessions"
    slug = next(sessions.iterdir(), None) if sessions.is_dir() else None
    if slug is None:
        print("no session directory was written; check the key and the model", file=sys.stderr)
        return 1
    # The session files are the record evals/axes.py scores; the temp HOME is not kept.
    shutil.copytree(slug, out / "sessions", dirs_exist_ok=True)
    extractor = ROOT / "skills/yi/session-mining/extract.py"
    return subprocess.run(
        [sys.executable, str(extractor), "--sessions", str(slug), "--out", str(out / "mining")],
        check=False,
    ).returncode


if __name__ == "__main__":
    sys.exit(main())
