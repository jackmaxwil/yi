#!/usr/bin/env python3
"""Delete the build artifacts no gate command would reuse (`just sweep`).

Incident: one worktree's target/ went 3.9 -> 6.5 GB in a few edit cycles. Cargo never
prunes the hashed artifacts of a superseded unit, and rustc keeps one incremental
directory per superseded crate hash.
"""
import json
import os
import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(subprocess.check_output(["git", "rev-parse", "--show-toplevel"], text=True).strip())
TARGET = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "target"))
# The resolutions a gate or the dev loop builds: the workspace tests, the shipped-tables
# check (D263's alone-resolution), `just dev`, and clippy. Anything else is stale.
PASSES = [
    ["cargo", "test", "--workspace", "--no-run"],
    ["cargo", "test", "-p", "yi-cli", "-p", "yi-tui", "-p", "yi-tools", "--test", "integration", "--no-run"],
    ["cargo", "build", "-p", "yi-cli"],
    ["cargo", "clippy", "--workspace", "--all-targets"],
]
HASH = re.compile(r"-([0-9a-f]{16})(?:\.|$)")


def size(path: Path) -> int:
    if path.is_file() or path.is_symlink():
        return path.lstat().st_size
    return sum(f.lstat().st_size for f in path.rglob("*") if f.is_file())


def live_units() -> dict[str, set[str]]:
    live: dict[str, set[str]] = {}
    for command in PASSES:
        run = subprocess.run(command + ["--message-format", "json"], cwd=ROOT, capture_output=True, text=True)
        if run.returncode != 0:
            sys.exit(f"sweep: `{' '.join(command)}` failed, nothing deleted:\n{run.stderr[-2000:]}")
        for line in run.stdout.splitlines():
            try:
                message = json.loads(line)
            except ValueError:
                continue
            if message.get("reason") != "compiler-artifact":
                continue
            for name in [*message.get("filenames", []), message.get("executable") or ""]:
                found = HASH.search(os.path.basename(name))
                if found:
                    crate = message.get("target", {}).get("name", "").replace("-", "_")
                    live.setdefault(crate, set()).add(found.group(1))
    return live


def remove(path: Path) -> int:
    freed = size(path)
    shutil.rmtree(path) if path.is_dir() and not path.is_symlink() else path.unlink()
    return freed


def main() -> None:
    live = live_units()
    hashes = {h for units in live.values() for h in units}
    freed = 0
    deps = TARGET / "debug" / "deps"
    for entry in deps.iterdir() if deps.is_dir() else []:
        found = HASH.search(entry.name)
        if found and found.group(1) not in hashes:
            freed += remove(entry)
    # An incremental directory is `<crate>-<hash>` with a hash of rustc's own, so it cannot be
    # matched to a unit: keep, per crate, as many of the newest as it has live units.
    incremental = TARGET / "debug" / "incremental"
    named: dict[str, list[Path]] = {}
    for entry in incremental.iterdir() if incremental.is_dir() else []:
        named.setdefault(entry.name.rsplit("-", 1)[0], []).append(entry)
    for crate, entries in named.items():
        entries.sort(key=lambda entry: entry.stat().st_mtime, reverse=True)
        for entry in entries[len(live.get(crate, ())):]:
            freed += remove(entry)
    print(f"sweep: kept {len(hashes)} live unit hashes, freed {freed / 1e6:.0f} MB under {TARGET}")


if __name__ == "__main__":
    main()
