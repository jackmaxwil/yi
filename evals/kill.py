#!/usr/bin/env python3
"""Named kill tests against a built yi, under one spend ledger.

    python3 evals/kill.py run <test> --binary <yi> [--family F] [--until ISO] [--dry]
    python3 evals/kill.py budget
    python3 evals/kill.py report

A test is its own module, evals/kill/<test>.py: THRESHOLD (the pre-registered text), FAMILIES
(route names in evals/kill/providers.json, run in that order), PREDICTED_USD ({family: dollars},
the plan's reserve, already plus 25%; a subscription family's figure is what the run would cost
on OpenRouter, reported when it cannot run), STEP (its place on the paid ladder), optional
DEADLINE (ISO time; a quota reset past it writes the run unrun), runs() (a list of run specs:
{"id", "prompt", "deadline", "args", "repo", "rev", "keep"}, or {"id", "harbor": argv}) and verdict(rows),
which sees only rows that ran: a row whose status is in NOT_RUN never reaches it.

Owner, 2026-10-23: "you can use my entire kimi subscription. then $30 openrouter cap". Every run
goes through ask(): a fresh HOME seeded with a clone of one prebuilt kernel venv per binary, a
fresh `git clone --shared` as cwd, a Kimi access token and never its refresh token, a live kill
when the stream's summed cost crosses what is left, and a park on an exhausted Kimi quota. The
ledger is ~/Development/yi-runs/next-shape/ledger.jsonl, flock'd and shared by every worktree.
Stdlib only.
"""
import argparse, contextlib, datetime, fcntl, hashlib, importlib.util, json, os, shutil
import subprocess, sys, threading, time, urllib.parse, urllib.request, uuid
from pathlib import Path

ROOT = Path(__file__).resolve().parent
sys.path.insert(0, str(ROOT / "adapters"))
sys.path.insert(0, str(ROOT / "drivers"))

import yi_usage  # noqa: E402
import trials  # noqa: E402
import watch  # noqa: E402

KILL_DIR = ROOT / "kill"
PROVIDERS = KILL_DIR / "providers.json"
TRIALS = ROOT / "trials"
CAP_USD = 30.0
QUOTA_MARKS = ("quota exhausted", "exceeded_current_quota_error", "insufficient_quota")
NOT_RUN = ("unrun", "crash", "quota")
GRACE_SEC = 60
# Injected by the selftest: a park is hours of wall clock.
clock, sleep = time.time, time.sleep


def runs_root():
    return Path(os.environ.get("EVAL_KILL_ROOT") or Path.home() / "Development" / "yi-runs")


def shape_dir():
    return runs_root() / "next-shape"


def micro(usd):
    """Dollars as integer micro-dollars: 29.73 + 0.27 is 30.000000000000004 in floats."""
    return round(usd * 1_000_000)


@contextlib.contextmanager
def flocked(path):
    path.parent.mkdir(parents=True, exist_ok=True)
    with open(path, "a+") as handle:
        fcntl.flock(handle, fcntl.LOCK_EX)
        yield handle


@contextlib.contextmanager
def ledger():
    """(rows, write) under the ledger's lock; the OS drops the lock with a dead holder."""
    with flocked(shape_dir() / "ledger.jsonl") as handle:
        handle.seek(0)
        rows = [json.loads(line) for line in handle if line.strip()]

        def write(row):
            handle.write(json.dumps({**row, "at": int(clock())}, sort_keys=True) + "\n")
            handle.flush()
            os.fsync(handle.fileno())
            rows.append(row)
        yield rows, write


def state(rows):
    """(spent micro-dollars, {key: (test, family, micro)} still reserved, {(test, family)} done,
    {(test, family)} with a row that ran)."""
    spent, held, done, ran = 0, {}, set(), set()
    for row in rows:
        if row["kind"] == "reserve":
            held[row["key"]] = (row["test"], row["family"], micro(row["usd"]))
        elif row["kind"] == "settle":
            spent += micro(row["usd"])
            held.pop(row["key"], None)
            if any(r.get("status") != "crash" for r in row["rows"]):
                done.add((row["test"], row["family"]))
            if any(r.get("status") not in NOT_RUN for r in row["rows"]):
                ran.add((row["test"], row["family"]))
        elif row["kind"] == "unrun":
            done.add((row["test"], row["family"]))
    return spent, held, done, ran


def admit(rows, test, family, usd, earlier=()):
    """Micro-dollars left after this reserve: spent, every open reserve, and the reserve of every
    earlier ladder step not yet settled stay within the cap, or the answer is negative."""
    spent, held, done, _ = state(rows)
    flying = {(t, f) for t, f, _ in held.values()}
    waiting = sum(micro(u) for t, f, u in earlier if (t, f) not in done | flying | {(test, family)})
    return micro(CAP_USD) - spent - sum(m for _, _, m in held.values()) - waiting - micro(usd)


def reserve(test, family, usd, earlier=()):
    """The reserve's key, or None after writing the test unrun with the dollars it needed."""
    with ledger() as (rows, write):
        left = admit(rows, test, family, usd, earlier)
        # Invariant: a $0 reserve is a subscription family, which the dollar cap never refuses.
        if left < 0 and usd > 0:
            had = (left + micro(usd)) / 1e6
            write({"kind": "unrun", "test": test, "family": family, "needsUsd": usd,
                   "why": f"needs ${usd:.2f}, ${max(had, 0):.2f} left of ${CAP_USD:g}"})
            return None
        key = f"{test}/{family}/{uuid.uuid4().hex[:12]}"
        write({"kind": "reserve", "key": key, "test": test, "family": family, "usd": usd, "pid": os.getpid()})
        return key


def settle(key, test, family, usd, rows):
    with ledger() as (_, write):
        write({"kind": "settle", "key": key, "test": test, "family": family, "usd": round(usd, 6), "rows": rows})


def routes():
    return json.loads(PROVIDERS.read_text())["routes"]


def refused(name, route):
    """Why a route may not spend, or None. A slug nobody confirmed is a run that 404s."""
    if route is None:
        return f"route {name} is not in {PROVIDERS.name}"
    if not route.get("enabled"):
        return f"route {name} is disabled: {route.get('why', '')}".rstrip(": ")
    if not route.get("confirmed"):
        return f"route {name} ({route['model']}) is not confirmed: {route.get('why', '')}".rstrip(": ")
    provider, slug = route["model"].split("/", 1)
    catalog = kimi_home() / ".yi" / "catalog" / f"{provider}.json"
    # ponytail: a substring test on the catalog file; parse it if a slug ever collides.
    if not catalog.exists() or json.dumps(slug) not in catalog.read_text():
        return f"route {name} ({route['model']}) is not in {catalog}: HOME={kimi_home()} yi catalog refresh {provider}"
    return None


def paid(route):
    return route.get("kind") != "subscription"


def kimi_home():
    """The HOME the owner logs Kimi into, once: `HOME=<this> yi login kimi --no-browser`."""
    return shape_dir() / "kimi-home"


def write_0600(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    tmp = path.with_name(path.name + ".tmp")
    tmp.unlink(missing_ok=True)
    fd = os.open(tmp, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
    with os.fdopen(fd, "w") as handle:
        json.dump(value, handle)
    os.replace(tmp, path)


def access_token(provider, need_sec):
    """The central credential's access token, refreshed under the lock yi's own refresh takes
    when it would expire inside need_sec. Copies of a rotating refresh token in parallel HOMEs
    would invalidate each other, so a run HOME gets the access token alone."""
    yi = kimi_home() / ".yi"
    path = yi / "providers" / "tokens" / f"{provider}.json"
    with flocked(yi / "providers" / "locks" / f"{provider}.lock"):
        cred = json.loads(path.read_text())
        if cred.get("expires_ms") and cred["expires_ms"] / 1000 - clock() < need_sec:
            profile = json.loads((yi / "oauth" / f"{provider}.json").read_text())
            form = {"grant_type": "refresh_token", "refresh_token": cred["refresh"], "client_id": profile["client_id"]}
            if profile.get("client_secret"):
                form["client_secret"] = profile["client_secret"]
            request = urllib.request.Request(profile["token"], data=urllib.parse.urlencode(form).encode(),
                                             headers={"Accept": "application/json"})
            with urllib.request.urlopen(request, timeout=60) as response:
                got = json.load(response)
            cred = {**cred, "access": got["access_token"], "refresh": got.get("refresh_token") or cred["refresh"],
                    "expires_ms": int((clock() + float(got["expires_in"])) * 1000)}
            write_0600(path, cred)
    left = cred["expires_ms"] / 1000 - clock() if cred.get("expires_ms") else float("inf")
    if left < need_sec:
        raise RuntimeError(f"the {provider} access token lives {left:.0f} s and a run needs {need_sec} s")
    return {"version": 1, "kind": "oauth", "access": cred["access"], "expires_ms": cred.get("expires_ms", 0)}


_SEEDS, _SEEDS_LOCK = {}, threading.Lock()


def prebuilt(binary):
    """One HOME holding a built kernel venv per binary identity: a fresh venv costs ~30 s."""
    with _SEEDS_LOCK:
        if binary not in _SEEDS:
            _SEEDS[binary] = hashlib.sha256(Path(binary).read_bytes()).hexdigest()[:12]
    seed = shape_dir() / "venvs" / _SEEDS[binary]
    with flocked(seed.with_name(seed.name + ".lock")):
        if not list((seed / ".yi").glob("kernel-venv-*")):
            seed.mkdir(parents=True, exist_ok=True)
            subprocess.run([binary, "doctor", "--fix", "--json"], env={**os.environ, "HOME": str(seed)},
                           stdin=subprocess.DEVNULL, capture_output=True, timeout=1800)
    return seed


def clone_tree(source, target):
    """An APFS clone (cp -c), a plain copy where the filesystem has none."""
    if subprocess.run(["cp", "-c", "-R", str(source), str(target)], capture_output=True).returncode:
        shutil.copytree(source, target, symlinks=True, dirs_exist_ok=True)


def seed_home(home, binary, route, need_sec):
    yi = home / ".yi"
    yi.mkdir(parents=True)
    routing = {yi_usage.ROUTING_ENV: json.dumps(route["routing"])} if route.get("routing") else {}
    (yi / "config.json").write_text(json.dumps(yi_usage.eval_config(routing)))
    for venv in sorted((prebuilt(binary) / ".yi").glob("kernel-venv-*")):
        clone_tree(venv, yi / venv.name)
    central = kimi_home() / ".yi"
    if (central / "catalog").is_dir():
        shutil.copytree(central / "catalog", yi / "catalog")
    provider = route["model"].split("/", 1)[0]
    # A --yolo run can read its HOME, so it gets its own route's key and no other provider's.
    key = central / "providers" / "tokens" / f"{provider}.json"
    if paid(route) and key.is_file() and json.loads(key.read_text()).get("kind") == "key":
        write_0600(yi / "providers" / "tokens" / key.name, json.loads(key.read_text()))
    if not paid(route):
        write_0600(yi / "providers" / "tokens" / f"{provider}.json", access_token(provider, need_sec))
        (yi / "oauth").mkdir()
        shutil.copy(central / "oauth" / f"{provider}.json", yi / "oauth" / f"{provider}.json")


def venv_ids(home):
    return {path.name: path.stat().st_ino for path in (home / ".yi").glob("kernel-venv-*")}


def quota_hit(events):
    for event in yi_usage.json_lines(events)[0]:
        message = event.get("message") if event.get("type") == "message_end" else None
        text = str((message or {}).get("errorMessage") or "")
        if "429" in text and any(mark in text.lower() for mark in QUOTA_MARKS):
            return True
    return False


def quota_reset(binary):
    """When the exhausted window resets, from `yi catalog usage kimi`; the latest reset of the
    windows at or past their limit, else the earliest of all; None when it cannot be read."""
    done = subprocess.run([binary, "catalog", "usage", "kimi"], capture_output=True, text=True, timeout=120,
                          env={**os.environ, "HOME": str(kimi_home())}, stdin=subprocess.DEVNULL)
    try:
        windows = [w for w in json.loads(done.stdout).values() if isinstance(w, dict) and w.get("reset")]
    except ValueError:
        return None
    at = lambda w: datetime.datetime.fromisoformat(w["reset"].replace("Z", "+00:00")).timestamp()
    full = [at(w) for w in windows if (w.get("used") or 0) >= 1]
    return max(full) if full else min(map(at, windows), default=None)


def park_file():
    return shape_dir() / "kimi-park.json"


def ask_once(test, family, route, spec, binary, stop_usd, ceiling_usd):
    run = f"{family}-{spec['id']}-{uuid.uuid4().hex[:8]}"
    base = runs_root() / test / run
    home, repo, events = base / "home", base / "repo", base / "events.jsonl"
    deadline = int(spec.get("deadline", 600))
    row = {"test": test, "family": family, "id": spec["id"], "run": run, "model": route["model"], "startedAt": clock()}
    try:
        seed_home(home, binary, route, deadline + 600)
        subprocess.run(["git", "clone", "--shared", "-q", str(spec.get("repo") or ROOT.parent), str(repo)], check=True)
        if spec.get("rev"):
            subprocess.run(["git", "-C", str(repo), "checkout", "-q", spec["rev"]], check=True)
        venvs = venv_ids(home)
        command = [binary, "ask", "--json", "--here", "--yolo", "--model", route["model"], "--session-dir",
                   str(base / "sessions"), "--deadline", str(deadline), *spec.get("args", ()), spec["prompt"]]
        started = time.monotonic()
        with (base / "stderr.txt").open("w") as errors:
            proc = subprocess.Popen(command, cwd=repo, env={**os.environ, "HOME": str(home)}, text=True,
                                    stdin=subprocess.DEVNULL, stdout=subprocess.PIPE, stderr=errors,
                                    start_new_session=True)
        fired = []
        timer = threading.Timer(deadline + GRACE_SEC, lambda: (fired.append(True), watch.kill_group(proc, 5)))
        timer.start()
        spent, status = 0.0, None
        with events.open("w") as sink, proc.stdout:
            for line in proc.stdout:
                if '"message_update"' in line:
                    continue
                sink.write(line)
                try:
                    event = json.loads(line)
                except ValueError:
                    continue
                message = event.get("message") if event.get("type") == "message_end" else None
                if not isinstance(message, dict) or message.get("role") != "assistant":
                    continue
                cost = ((message.get("usage") or {}).get("cost") or {}).get("total")
                spent += cost if isinstance(cost, (int, float)) else 0
                if paid(route) and status is None and spent > stop_usd:
                    status = "budget"
                    watch.kill_group(proc, 5)
        exit_code = proc.wait()
        timer.cancel()
        for name in spec.get("keep", ()):
            if (repo / name).is_file():
                shutil.copy(repo / name, base / name)
        usage = yi_usage.parse_events(events)
        unpriced = bool(usage["costUnknownTurns"])
        status = status or ("timeout" if fired else "quota" if quota_hit(events)
                            else "error" if exit_code else "ok")
        charged = 0.0 if not paid(route) else max(spent, min(stop_usd, ceiling_usd)) if unpriced else spent
        row.update(usage)
        row.update({"exit": exit_code, "status": status, "measuredUsd": round(spent, 6), "unpriced": unpriced,
                    "chargedUsd": round(charged, 6), "wallSec": round(time.monotonic() - started, 2),
                    "venvRebuilt": venv_ids(home) != venvs or not venvs,
                    "stderr": (base / "stderr.txt").read_text(errors="replace")[-2000:]})
        return row
    finally:
        shutil.rmtree(home, ignore_errors=True)
        shutil.rmtree(repo, ignore_errors=True)


def ask(test, family, spec, binary, stop_usd, until=None, estimate_usd=0.0, ceiling_usd=None):
    """The one budgeted call: a run row, charged its measured cost, or its per-run ceiling (at most
    stop_usd) when a turn was unpriced. An exhausted Kimi quota parks every Kimi run until the
    window resets; a reset past `until` writes the run unrun with its OpenRouter estimate."""
    route = routes()[family]
    if paid(route) and stop_usd <= 0:
        return {"test": test, "family": family, "id": spec["id"], "status": "unrun", "chargedUsd": 0.0,
                "needsUsd": estimate_usd, "why": "no reserve left"}
    for _ in range(3):
        parked = json.loads(park_file().read_text())["until"] if park_file().exists() and not paid(route) else 0
        if parked > clock():
            if until is not None and parked > until:
                return {"test": test, "family": family, "id": spec["id"], "status": "unrun", "chargedUsd": 0.0,
                        "needsUsd": estimate_usd, "why": f"quota resets at {parked:.0f}, past the deadline {until:.0f}"}
            sleep(parked - clock())
        row = ask_once(test, family, route, spec, binary, stop_usd,
                       stop_usd if ceiling_usd is None else ceiling_usd)
        if row["status"] != "quota":
            return row
        row = {**row, "needsUsd": estimate_usd, "why": "the quota stayed exhausted"}
        if paid(route):
            return row
        reset = quota_reset(binary)
        if reset is None:
            return {**row, "why": "quota exhausted and `yi catalog usage kimi` gave no reset"}
        with flocked(shape_dir() / "kimi-park.lock"):
            write_0600(park_file(), {"until": reset})
    return row


def harbor(test, family, spec, stop_usd):
    """A harbor sweep holds its own caps (watch.py); its hard cap is what is left of the reserve.
    Only `tbv4_sweep.sh --runner` files rows, so a sweep that filed none is charged that cap."""
    run = f"ns-{test}-{family}-{spec['id']}"
    if not paid(routes()[family]):
        return {"test": test, "family": family, "id": spec["id"], "status": "unrun", "chargedUsd": 0.0,
                "needsUsd": 0.0, "why": "a harbor sweep spends OpenRouter on its own model, never a subscription"}
    env = {**os.environ, "TBV4_HARD_CAP": f"{max(stop_usd, 0):.2f}", "EVAL_RUN_ID": run}
    store = TRIALS / f"{run}.jsonl"
    stored = lambda: [json.loads(line) for line in store.read_text().splitlines() if line.strip()] if store.exists() else []
    before = len(stored())
    done = subprocess.run(spec["harbor"], cwd=ROOT.parent, env=env)
    # The store is appended per run id, so a re-run charges only the rows its own sweep filed.
    filed = stored()[before:]
    charged = sum(trials.cost(row) for row in filed) if filed else stop_usd
    return {"test": test, "family": family, "run": run, "status": "ok" if done.returncode == 0 else "error",
            "exit": done.returncode, "trials": len(filed), "chargedUsd": round(charged, 6)}


def tests():
    found = {}
    for path in sorted(KILL_DIR.glob("*.py")):
        spec = importlib.util.spec_from_file_location(f"kill_{path.stem}", path)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        found[path.stem] = module
    return found


def reserve_of(module, family, route):
    return module.PREDICTED_USD.get(family, 0.0) if paid(route) else 0.0


def earlier_steps(name, module, table):
    """(test, family, reserve) of every paid family on a ladder step before this one."""
    out = []
    for other, mod in tests().items():
        if other != name and mod.STEP < module.STEP:
            out += [(other, f, reserve_of(mod, f, table.get(f, {}))) for f in mod.FAMILIES if table.get(f)]
    return out


def when(text):
    return datetime.datetime.fromisoformat(text.replace("Z", "+00:00")).timestamp() if text else None


def cmd_run(args):
    available = tests()
    if args.test not in available:
        print(f"no test {args.test} in {KILL_DIR}", file=sys.stderr)
        return 2
    module, table = available[args.test], routes()
    families = [args.family] if args.family else list(module.FAMILIES)
    problems = [refused(f, table.get(f)) for f in families]
    if args.family and args.family not in module.FAMILIES:
        problems.append(f"{args.test} has no family {args.family}; its families are {', '.join(module.FAMILIES)}")
    gate = refused("kimi", table.get("kimi"))
    if gate and not args.without_kimi and any(paid(table.get(f, {})) for f in families):
        problems.append(f"a paid run waits for the Kimi route (--without-kimi overrides): {gate}")
    if any(problems):
        print("refused: " + "; ".join(p for p in problems if p), file=sys.stderr)
        return 2
    specs, earlier = module.runs(), earlier_steps(args.test, module, table)
    until = when(args.until) or when(getattr(module, "DEADLINE", None))
    code = 0
    for family in families:
        route, usd = table[family], reserve_of(module, family, table[family])
        with ledger() as (rows, _):
            ran = state(rows)[3]
            left = admit(rows, args.test, family, usd, earlier)
        first = [f for f in module.FAMILIES[: list(module.FAMILIES).index(family)] if (args.test, f) not in ran]
        if first and paid(route) and not (args.dry or args.without_kimi):
            print(f"refused: {args.test} runs {', '.join(first)} before {family}", file=sys.stderr)
            return 2
        if args.dry:
            print(json.dumps({"test": args.test, "family": family, "model": route["model"], "runs": len(specs),
                              "reserveUsd": usd, "admitted": usd == 0 or left >= 0, "leftUsd": left / 1e6}))
            continue
        key = reserve(args.test, family, usd, earlier)
        if key is None:
            print(f"{args.test} {family}: unrun, needs ${usd:.2f}")
            code = 1
            continue
        rows, spent = [], 0.0
        estimate, share = (module.PREDICTED_USD.get(family, 0.0) / max(len(specs), 1), usd / max(len(specs), 1))
        try:
            for spec in specs:
                left_usd = usd - spent if paid(route) else float("inf")
                try:
                    row = (harbor(args.test, family, spec, left_usd) if "harbor" in spec else
                           ask(args.test, family, spec, args.binary, left_usd, until, estimate, share))
                except Exception as error:  # noqa: BLE001 - a missing login or a bad clone is a row, not a stop
                    row = {"test": args.test, "family": family, "id": spec["id"], "status": "crash",
                           "chargedUsd": max(min(share, left_usd), 0.0) if paid(route) else 0.0, "needsUsd": estimate,
                           "why": f"{type(error).__name__}: {error}"[-2000:]}
                rows.append(row)
                spent += row["chargedUsd"]
        finally:
            settle(key, args.test, family, spent, rows)
            TRIALS.mkdir(exist_ok=True)
            with (TRIALS / f"ns-{args.test}.jsonl").open("a") as sink:
                sink.writelines(json.dumps(row, sort_keys=True) + "\n" for row in rows)
        missed = [row for row in rows if row["status"] in NOT_RUN]
        line = f"{args.test} {family}: {len(rows) - len(missed)} runs, ${spent:.4f}"
        if missed:
            line += f", {len(missed)} not run, needs ${sum(r.get('needsUsd', 0.0) for r in missed):.2f}: {missed[0]['why']}"
            code = 1
        print(line)
    return code


def cmd_budget(_args):
    table = routes()
    with ledger() as (rows, _):
        spent, held, done, _ = state(rows)
    ladder = [{"step": mod.STEP, "test": name, "family": f, "reserveUsd": reserve_of(mod, f, table[f]),
               "settled": (name, f) in done} for name, mod in tests().items() for f in mod.FAMILIES if f in table]
    print(json.dumps({"capUsd": CAP_USD, "spentUsd": spent / 1e6, "heldUsd": sum(m for *_, m in held.values()) / 1e6,
                      "leftUsd": (micro(CAP_USD) - spent - sum(m for *_, m in held.values())) / 1e6,
                      "open": sorted(held), "ladder": sorted(ladder, key=lambda r: r["step"])}, indent=1))
    return 0


def cmd_report(_args):
    """One line per test module: its verdict over the rows that ran, naming every family whose
    latest record did not run and the dollars it needs; verdict() never sees an unrun row."""
    with ledger() as (rows, _):
        pass
    for name, module in sorted(tests().items(), key=lambda item: item[1].STEP):
        ran, missing = [], {}
        for family in module.FAMILIES:
            mine = [row for row in rows if row["test"] == name and row["family"] == family and row["kind"] != "reserve"]
            ran += [r for row in mine if row["kind"] == "settle" for r in row["rows"] if r.get("status") not in NOT_RUN]
            last = mine[-1] if mine else {"kind": "settle", "rows": []}
            lost = [last] if last["kind"] == "unrun" else [r for r in last["rows"] if r.get("status") in NOT_RUN]
            if lost or not last["rows"]:
                missing[family] = sum(r.get("needsUsd", 0.0) for r in lost) if lost else module.PREDICTED_USD.get(family, 0.0)
        if not ran:
            line = f"unrun: needs ${sum(missing.values()):.2f}"
        else:
            gaps = ", ".join(f"{family} needs ${usd:.2f}" for family, usd in missing.items())
            line = str(module.verdict(ran)) + (f" (unrun: {gaps})" if missing else "")
        print(f"{name}: {line}")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    verbs = parser.add_subparsers(dest="verb", required=True)
    run = verbs.add_parser("run")
    run.add_argument("test")
    run.add_argument("--binary", default="target/debug/yi")
    run.add_argument("--family")
    run.add_argument("--until", help="ISO time; a Kimi quota reset past it writes the run unrun")
    run.add_argument("--dry", action="store_true", help="check routes and the ladder, spend nothing")
    run.add_argument("--without-kimi", action="store_true", help="run a paid family before Kimi is confirmed or has run")
    verbs.add_parser("budget")
    verbs.add_parser("report")
    args = parser.parse_args(argv)
    os.environ.setdefault("UV_CACHE_DIR", str(Path.home() / ".cache" / "uv"))
    return {"run": cmd_run, "budget": cmd_budget, "report": cmd_report}[args.verb](args)


if __name__ == "__main__":
    sys.exit(main())
