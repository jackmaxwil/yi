import os
import pathlib
import subprocess
import sys

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import forge_pr  # noqa: E402
import pr_review  # noqa: E402


def nextest(cwd, expr, extra_env, runner):
    env = {**os.environ, **extra_env}
    command = ["scripts/wt_home.sh", "cargo", "nextest", "run", "--no-fail-fast", "-E", expr]
    return runner(command, cwd=cwd, env=env, check=False).returncode


def main(expr, base=None, runner=subprocess.run):
    base = base or forge_pr.git("merge-base", "origin/main", "HEAD", check=True).strip()
    with pr_review.scratch_worktree(forge_pr.git, base, "yi-baseline-") as tree:
        print(f"baseline: {base[:9]}")
        old = nextest(tree, expr, {"CARGO_TARGET_DIR": str(tree.parent / "target")}, runner)
    print(f"head: {forge_pr.git('rev-parse', '--short', 'HEAD').strip()}")
    new = nextest(forge_pr.ROOT, expr, {}, runner)
    print(f"baseline exit {old}, head exit {new}")
    return new


if __name__ == "__main__":
    sys.exit(main(sys.argv[1]))
