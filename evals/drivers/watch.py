#!/usr/bin/env python3
"""Run a harbor sweep as a child and hold its caps while it runs.

    python3 evals/drivers/watch.py --runs DIR --hard USD --wall SECONDS -- harbor run ...

harbor bind-mounts each trial's /logs/agent, so every trial's yi.jsonl grows on the host.
Every poll:
  - a trial past $1 or 180 turns: stop that trial's containers only (rule 7) and write
    `<trial>/censored`, so the gate never reads what the stop left as the trial's own result;
  - a trial with an unpriced turn (D79) counts $1, the per-trial cap it cannot pass, toward
    the run's total: never $0 and never a local price table (E14);
  - the run's total past --hard, the wall past --wall, or host space under 8 GB free for
    important usage (3 GB plain):
    stop the child's whole process group and this run's trial containers, exit 2;
  - remove terminal-bench images no container uses. harbor's `--rmi local` leaves pulled
    images behind, and they are pulled by digest, so they carry no tag and go by id.
The reason for a stop goes to <runs>.STOPPED; one log line per poll on stdout. A finished
child exits with its own code.
"""

import argparse
import os
import shutil
import signal
import subprocess
import sys
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
sys.path.insert(0, str(ROOT / "adapters"))

import yi_usage  # noqa: E402

PER_TRIAL_USD = 1.0
TURN_CAP = 180
MIN_FREE_GB = 8
MIN_PLAIN_GB = 3
# Incident (PAID-0, 2026-09-27): freight-dispatch-shift pulls a sidecar image, then its main
# image for minutes; the sidecar sat unused for two polls, was removed, and the trial died on
# "No such image". An image goes after this many consecutive unused polls, not two.
IDLE_POLLS = 10
IMPORTANT_USAGE = ('ObjC.import("Foundation"); var r = Ref(); $.NSURL.fileURLWithPath("{}")'
                   '.getResourceValueForKeyError(r, $.NSURLVolumeAvailableCapacityForImportantUsageKey, null); r[0].js')


def free_gb(path):
    """(free for important usage, plain free). Incident: macOS holds Time Machine local
    snapshots as used until an app asks for the space, then purges them; mid-sweep df said
    12.8 GB while 61.9 GB was available, and the plain figure would have stopped the run."""
    plain = shutil.disk_usage(path).free / 1e9
    try:
        out = subprocess.run(("osascript", "-l", "JavaScript", "-e", IMPORTANT_USAGE.format(path.resolve())),
                             capture_output=True, text=True, timeout=30).stdout.strip()
        return float(out) / 1e9, plain
    except (OSError, ValueError, subprocess.TimeoutExpired):
        return plain, plain


def docker(*args):
    # The driver preflights docker; a host without it has no container to stop.
    try:
        return subprocess.run(("docker",) + args, capture_output=True, text=True, timeout=120).stdout
    except FileNotFoundError:
        return ""


def trials(runs):
    """(trial dir, cost, turns); an unpriced trial's cost is PER_TRIAL_USD, its upper bound."""
    for path in runs.rglob("agent/yi.jsonl"):
        usage = yi_usage.parse_events(path)
        cost = PER_TRIAL_USD if usage.get("costUnknownTurns") else usage.get("costUsd") or 0.0
        yield path.parents[1], cost, usage.get("nAssistantMessages") or 0


def stop_containers(trial_names):
    """Only this run's trials: harbor names a trial's containers `<trial>__<service>`, lowercased.
    Incident: a stop that took every `__` container killed three trials of a sweep running beside
    the watcher's own smoke test."""
    prefixes = tuple(name.lower() + "__" for name in trial_names)
    for name in docker("ps", "--format", "{{.Names}}").split():
        if prefixes and name.lower().startswith(prefixes):
            docker("stop", "-t", "5", name)


def prune(idle, ripe_at=IDLE_POLLS):
    """An image goes only after IDLE_POLLS consecutive polls unused (`idle` counts them per id):
    a multi-image task pulls its sidecar long before it creates the container.
    `docker rmi` without -f refuses any image a container references. Once harbor has exited,
    nothing will create another container, so the final prune takes every unused image."""
    ids = set()
    for line in docker("images", "--format", "{{.Repository}} {{.ID}}").splitlines():
        repo, _, image_id = line.partition(" ")
        if "terminal-bench" in repo:
            ids.add(image_id)
    used = {docker("inspect", "--format", "{{.Image}}", cid).strip().removeprefix("sha256:")[:12]
            for cid in docker("ps", "-aq").split()}
    unused = ids - used
    for image in list(idle):
        if image not in unused:
            del idle[image]
    for image in unused:
        idle[image] = idle.get(image, 0) + 1
    ripe = [image for image, polls in idle.items() if polls >= ripe_at]
    removed = sum(not subprocess.run(("docker", "rmi", i), capture_output=True).returncode for i in ripe)
    for image in ripe:
        idle.pop(image, None)
    return removed


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--runs", type=Path, required=True)
    parser.add_argument("--hard", type=float, required=True)
    parser.add_argument("--wall", type=float, required=True)
    parser.add_argument("--poll", type=float, default=60)
    parser.add_argument("command", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    command = args.command[1:] if args.command[:1] == ["--"] else args.command
    if not command:
        parser.error("no command after --")
    args.runs.mkdir(parents=True, exist_ok=True)
    stopped = args.runs.parent / (args.runs.name + ".STOPPED")
    child = subprocess.Popen(command, start_new_session=True)
    started, breached, unused_last = time.monotonic(), set(), {}
    while True:
        try:
            code = child.wait(timeout=args.poll)
        except subprocess.TimeoutExpired:
            code = None
        if code is not None:
            print("child exited", code, "; final prune", prune(unused_last, ripe_at=0), flush=True)
            return code
        total = 0.0
        for trial, cost, turns in trials(args.runs):
            total += cost
            if trial.name not in breached and (cost > PER_TRIAL_USD or turns > TURN_CAP):
                breached.add(trial.name)
                print(f"TRIAL STOP {trial.name}: ${cost:.3f} at {turns} turns", flush=True)
                (trial / "censored").write_text(f"stopped past ${PER_TRIAL_USD:g} or {TURN_CAP} turns: "
                                                f"${cost:.3f} at {turns} turns\n")
                stop_containers([trial.name])
        free, plain = free_gb(args.runs)
        elapsed = time.monotonic() - started
        print(f"{time.strftime('%H:%M:%S')} total ${total:.3f} free {free:.1f} GB (plain {plain:.1f}) "
              f"elapsed {elapsed / 3600:.2f} h pruned {prune(unused_last)}", flush=True)
        reason = None
        if total > args.hard:
            reason = f"spend ${total:.3f} passed the hard cap ${args.hard:.2f}"
        elif elapsed > args.wall:
            reason = f"wall {elapsed:.0f} s passed {args.wall:.0f} s"
        elif free < MIN_FREE_GB or plain < MIN_PLAIN_GB:
            reason = f"host free space {free:.1f} GB (plain {plain:.1f}) under {MIN_FREE_GB} ({MIN_PLAIN_GB}) GB"
        if reason:
            stopped.write_text(reason + "\n")
            print("STOP:", reason, flush=True)
            for sig in (signal.SIGTERM, signal.SIGKILL):
                try:
                    os.killpg(child.pid, sig)
                except ProcessLookupError:
                    break
                try:
                    child.wait(timeout=120)
                    break
                except subprocess.TimeoutExpired:
                    continue
            stop_containers(path.name for path in args.runs.glob("*/*__*") if path.is_dir())
            return 2


if __name__ == "__main__":
    sys.exit(main())
