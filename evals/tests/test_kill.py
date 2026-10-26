"""The kill-test runner's spend ledger, live kill, quota park and run isolation, against a fake
yi that replays real `yi ask --json` streams: no model, no network past 127.0.0.1, no paid run."""
import contextlib, datetime, http.server, io, json, os, subprocess, sys, tempfile, threading, unittest
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))
import kill  # noqa: E402
import trials  # noqa: E402

EVENTS = EVALS / "fixtures" / "ask_events.jsonl"
# Main's binary against a 127.0.0.1 fake answering Moonshot's exceeded_current_quota_error 429.
QUOTA = EVALS / "fixtures" / "ask_quota_429.jsonl"

FAKE_YI = r'''#!/usr/bin/env python3
import fcntl, json, os, sys, time
from pathlib import Path
home, log, argv = Path(os.environ["HOME"]), Path(os.environ["FAKE_LOG"]), sys.argv[1:]
def note(**row):
    with open(log, "a") as sink:
        sink.write(json.dumps(row) + "\n")
if argv[:2] == ["doctor", "--fix"]:
    (home / ".yi" / ("kernel-venv-fake" if not os.environ.get("FAKE_NO_VENV") else "no-venv")).mkdir(parents=True)
    note(build="doctor")
    print("[]")
elif argv[:3] == ["login", "kimi", "--refresh"]:
    lock = home / ".yi" / "providers" / "locks" / "kimi.lock"
    lock.parent.mkdir(parents=True, exist_ok=True)
    with open(lock, "a") as held:
        fcntl.flock(held, fcntl.LOCK_EX)
        central = home / ".yi" / "providers" / "tokens" / "kimi.json"
        credential = json.loads(central.read_text())
        if credential["expires_ms"] / 1000 - time.time() < int(argv[3]):
            credential.update(access="fresh-access", refresh="rotated", expires_ms=int((time.time() + 7200) * 1000))
            central.write_text(json.dumps(credential))
            note(rotated=True)
    note(refresh=argv)
    print(json.dumps({"version": 1, "kind": "oauth", "access": credential["access"], "expires_ms": credential["expires_ms"]}))
elif argv[:3] == ["catalog", "usage", "kimi"]:
    print(os.environ["FAKE_USAGE"])
elif argv[:1] == ["ask"]:
    venvs = sorted(p.name for p in (home / ".yi").glob("kernel-venv-*"))
    if not venvs:
        (home / ".yi" / "kernel-venv-fake").mkdir(parents=True)
        note(build="boot")
    token = home / ".yi" / "providers" / "tokens" / "kimi.json"
    asked = sum(1 for line in open(log) if '"ask"' in line) if log.exists() else 0
    note(ask=os.getcwd(), home=str(home), venvs=venvs, git=Path(".git").is_dir(),
         config=json.loads((home / ".yi" / "config.json").read_text()),
         token=json.loads(token.read_text()) if token.exists() else None,
         mode=oct(token.stat().st_mode & 0o777) if token.exists() else None)
    sys.stderr.write("fake stderr: " + "x" * 3000 + " the tail\n")
    streams = os.environ["FAKE_STREAMS"].split(os.pathsep)
    for line in open(streams[min(asked, len(streams) - 1)]):
        sys.stdout.write(line)
        sys.stdout.flush()
        time.sleep(float(os.environ.get("FAKE_PACE", "0")))
'''

ROUTES = {"routes": {
    "kimi": {"model": "kimi/k3.1", "kind": "subscription", "enabled": True, "confirmed": True},
    "flash": {"model": "openrouter/z-ai/glm-5.3-flash", "kind": "openrouter", "enabled": True, "confirmed": True,
              "routing": {"ignore": ["inference-net"]}},
    "dark": {"model": "openrouter/deepseek/nope", "kind": "openrouter", "enabled": True, "confirmed": False}}}

MODULE = '''THRESHOLD = "fixture"
FAMILIES = {families!r}
PREDICTED_USD = {predicted!r}
STEP = {step}
def runs():
    return [{{"id": f"r{{n}}", "prompt": "Summarize the number two.", "deadline": 60, "repo": {repo!r}}}
            for n in range({count})]
def verdict(rows):
    return "KEEP" if all(row["status"] == "ok" for row in rows) else "KILL"
'''


def costed(path, turns, cost, unknown=False):
    """The recorded faux stream with its assistant turn repeated and priced: faux bills $0."""
    lines = EVENTS.read_text().splitlines()
    body, end = lines[:-1], lines[-1]
    out = []
    for _ in range(turns):
        for line in body:
            event = json.loads(line)
            message = event.get("message") or {}
            if event.get("type") in ("message_end", "turn_end") and message.get("role") == "assistant":
                message["usage"]["cost"]["total"] = cost
                message["usage"]["unknown"] = unknown
            out.append(json.dumps(event))
    path.write_text("\n".join(out + [end]) + "\n")
    return path


class Rig:
    """A runs root, a providers file, a kill dir and a fake yi, swapped into the module."""

    def __init__(self, tmp):
        self.tmp = Path(tmp).resolve()
        self.saved = (kill.PROVIDERS, kill.KILL_DIR, kill.TRIALS, kill.clock, kill.sleep, kill.GRACE_SEC,
                      dict(os.environ))
        self.root = self.tmp / "yi-runs"
        os.environ["EVAL_KILL_ROOT"] = str(self.root)
        kill.PROVIDERS = self.tmp / "providers.json"
        kill.PROVIDERS.write_text(json.dumps(ROUTES))
        kill.KILL_DIR = self.tmp / "kill"
        kill.KILL_DIR.mkdir()
        kill.TRIALS = self.tmp / "trials"
        self.log = self.tmp / "fake.log"
        self.yi = self.tmp / "yi"
        self.yi.write_text(FAKE_YI)
        self.yi.chmod(0o755)
        os.environ["FAKE_LOG"] = str(self.log)
        self.repo = self.tmp / "repo"
        self.repo.mkdir()
        subprocess.run(["git", "init", "-q", str(self.repo)], check=True)
        subprocess.run(["git", "-C", str(self.repo), "-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q",
                        "--allow-empty", "-m", "seed"], check=True)
        self.spec = {"id": "one", "prompt": "Summarize the number two.", "deadline": 60, "repo": str(self.repo)}
        self.catalog = kill.kimi_home() / ".yi" / "catalog"
        self.catalog.mkdir(parents=True)
        (self.catalog / "openrouter.json").write_text('{"models": [{"id": "z-ai/glm-5.3-flash"}]}')
        (self.catalog / "kimi.json").write_text('{"models": [{"id": "k3.1"}]}')

    def restore(self):
        kill.PROVIDERS, kill.KILL_DIR, kill.TRIALS, kill.clock, kill.sleep, kill.GRACE_SEC, environ = self.saved
        os.environ.clear()
        os.environ.update(environ)

    def streams(self, *paths, pace=0.0):
        os.environ["FAKE_STREAMS"] = os.pathsep.join(map(str, paths))
        os.environ["FAKE_PACE"] = str(pace)

    def module(self, name, families, predicted, step=1, count=1):
        (kill.KILL_DIR / f"{name}.py").write_text(MODULE.format(families=families, predicted=predicted, step=step,
                                                                repo=str(self.repo), count=count))

    def notes(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []

    def ledger(self):
        path = self.root / "next-shape" / "ledger.jsonl"
        return [json.loads(line) for line in path.read_text().splitlines()] if path.exists() else []

    def spend(self, usd):
        with kill.ledger() as (_, write):
            write({"kind": "settle", "key": "before", "test": "earlier", "family": "flash", "usd": usd, "rows": []})

    def kimi_login(self, expires_in, token_url="http://127.0.0.1:9/token"):
        yi = kill.kimi_home() / ".yi"
        kill.write_0600(yi / "oauth" / "kimi.json", {"version": 1, "kind": "device-code", "client_id": "fixture",
                                                     "token": token_url})
        kill.write_0600(yi / "providers" / "tokens" / "kimi.json",
                        {"version": 1, "kind": "oauth", "access": "old-access", "refresh": "rotating-refresh",
                         "expires_ms": int((kill.time.time() + expires_in) * 1000)})


@contextlib.contextmanager
def rig():
    with tempfile.TemporaryDirectory(prefix="kill-") as tmp:
        built = Rig(tmp)
        try:
            yield built
        finally:
            built.restore()


def quiet(argv):
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        code = kill.main(argv)
    return code, out.getvalue()


class Ledger(unittest.TestCase):
    def test_a_test_runs_at_the_cap_and_one_cent_more_is_written_unrun_with_its_dollars(self):
        for predicted, runs in ((0.27, 1), (0.28, 0)):
            with rig() as r:
                r.streams(costed(r.tmp / "s.jsonl", 1, 0.01))
                r.spend(29.73)
                r.module("capped", ["flash"], {"flash": predicted})
                code, out = quiet(["run", "capped", "--binary", str(r.yi)])
                self.assertEqual(len([n for n in r.notes() if "ask" in n]), runs, f"spent 29.73, predicted {predicted}")
                self.assertEqual(code, 1 - runs, out)
                unrun = [row["why"] for row in r.ledger() if row["kind"] == "unrun"]
                self.assertEqual(unrun, [] if runs else ["needs $0.28, $0.27 left of $30"])

    def test_a_reserve_whose_runner_died_is_closed_by_hand_at_its_full_reserve(self):
        with rig() as r:
            r.module("hung", ["flash"], {"flash": 0.4})
            dead = subprocess.Popen([sys.executable, "-c", "pass"])
            dead.wait()
            with kill.ledger() as (_, write):
                write({"kind": "reserve", "key": "hung/flash/dead", "test": "hung", "family": "flash", "usd": 0.4,
                       "pid": dead.pid})
                write({"kind": "reserve", "key": "hung/flash/live", "test": "hung", "family": "flash", "usd": 0.3,
                       "pid": os.getpid()})
            self.assertEqual(quiet(["budget", "--close", "hung/flash/live"]),
                             (2, f"refused: hung/flash/live's runner, pid {os.getpid()}, is alive and settles it itself\n"))
            code, out = quiet(["budget", "--close", "hung/flash/dead"])
            self.assertEqual(code, 0, out)
            shown = json.loads(out)
            self.assertEqual((shown["spentUsd"], shown["heldUsd"], shown["open"]), (0.4, 0.3, ["hung/flash/live"]))
            self.assertEqual(quiet(["budget", "--close", "hung/flash/dead"])[0], 2, "a closed reserve closes once")
            self.assertEqual(quiet(["report"])[1].strip(), "hung: unrun: needs $0.40", "a closed reserve never ran")

    def test_twelve_threads_and_another_process_never_reserve_past_the_cap(self):
        with rig() as r:
            path = r.root / "next-shape" / "ledger.jsonl"
            path.parent.mkdir(parents=True, exist_ok=True)
            held = r.tmp / "held"
            holder = subprocess.Popen([sys.executable, "-c", (
                "import fcntl, json, sys, time, pathlib\n"
                "with open(sys.argv[1], 'a+') as h:\n"
                "    fcntl.flock(h, fcntl.LOCK_EX); pathlib.Path(sys.argv[2]).touch(); time.sleep(0.5)\n"
                "    h.write(json.dumps({'kind': 'reserve', 'key': 'other', 'test': 'other', 'family': 'flash',"
                " 'usd': 5.0}) + '\\n')\n"), str(path), str(held)])
            while not held.exists():
                kill.time.sleep(0.01)
            keys, gate = [], threading.Barrier(12)

            def one(n):
                gate.wait()
                keys.append(kill.reserve(f"t{n}", "flash", 3.0))
            threads = [threading.Thread(target=one, args=(n,)) for n in range(12)]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join()
            holder.wait()
            with kill.ledger() as (rows, _):
                spent, open_, *_ = kill.state(rows)
            self.assertIn("other", open_, "the other process's reserve must be on the ledger")
            self.assertLessEqual(sum(m for *_, m in open_.values()), kill.micro(30.0), open_)
            self.assertEqual(sum(key is not None for key in keys), 8, "5 + 8 x 3 is 29; a ninth would be 32")

    def test_a_step_waits_behind_the_reserve_of_an_earlier_unsettled_step(self):
        with rig() as r:
            r.spend(20.0)
            earlier = [("first", "flash", 6.0)]
            self.assertIsNone(kill.reserve("second", "flash", 5.0, earlier), "20 + 6 held + 5 passes 30")
            self.assertIsNotNone(kill.reserve("second", "flash", 4.0, earlier), "20 + 6 + 4 is the cap")

    def test_a_stream_that_crosses_its_reserve_is_killed_and_charged_what_it_measured(self):
        with rig() as r:
            r.streams(costed(r.tmp / "s.jsonl", 6, 0.10), pace=0.02)
            row = kill.ask("t", "flash", r.spec, str(r.yi), 0.25)
            self.assertEqual(row["status"], "budget", row)
            self.assertAlmostEqual(row["measuredUsd"], 0.30, places=6)
            self.assertAlmostEqual(row["chargedUsd"], 0.30, places=6)

    def test_runs_after_a_budget_kill_are_written_unrun_and_the_command_says_so(self):
        with rig() as r:
            r.streams(costed(r.tmp / "s.jsonl", 6, 0.10), pace=0.02)
            r.module("two", ["flash"], {"flash": 0.25}, count=2)
            code, out = quiet(["run", "two", "--binary", str(r.yi)])
            self.assertEqual((code, out.strip()), (1, "two flash: 1 runs, $0.3000, 1 not run, needs $0.12: no reserve left"))

    def test_unpriced_runs_are_charged_their_share_so_the_rest_still_run(self):
        with rig() as r:
            r.streams(costed(r.tmp / "s.jsonl", 1, 0.0, unknown=True))
            r.module("four", ["flash"], {"flash": 1.0}, count=4)
            code, out = quiet(["run", "four", "--binary", str(r.yi)])
            self.assertEqual((code, out.strip()), (0, "four flash: 4 runs, $1.0000"), "a quarter of $1 per unpriced run")

    def test_an_unpriced_stream_is_charged_its_ceiling_never_zero(self):
        with rig() as r:
            r.streams(costed(r.tmp / "s.jsonl", 1, 0.0, unknown=True))
            row = kill.ask("t", "flash", r.spec, str(r.yi), 0.40)
            self.assertEqual((row["status"], row["unpriced"]), ("ok", True), row)
            self.assertEqual(row["chargedUsd"], 0.40, row)

    def test_dry_refuses_an_unconfirmed_route_by_name(self):
        with rig() as r:
            r.module("named", ["dark"], {"dark": 0.1})
            code, out = quiet(["run", "named", "--binary", str(r.yi), "--dry"])
            self.assertEqual(code, 2, out)
            self.assertIn("route dark (openrouter/deepseek/nope) is not confirmed", out)
            self.assertEqual(r.ledger(), [], "a dry run writes nothing")
            code, out = quiet(["run", "named", "--binary", str(r.yi), "--dry", "--family", "flash"])
            self.assertEqual((code, out.strip()), (2, "refused: named has no family flash; its families are dark"))

    def test_dry_plans_both_families_and_refuses_a_slug_the_central_catalog_lacks(self):
        with rig() as r:
            r.module("pair", ["kimi", "flash"], {"kimi": 0.5, "flash": 0.2})
            code, out = quiet(["run", "pair", "--binary", str(r.yi), "--dry"])
            self.assertEqual((code, [json.loads(line)["family"] for line in out.splitlines()]), (0, ["kimi", "flash"]), out)
            (r.catalog / "openrouter.json").write_text('{"models": []}')
            code, out = quiet(["run", "pair", "--binary", str(r.yi), "--dry"])
            self.assertEqual(code, 2, out)
            self.assertIn("yi catalog refresh openrouter", out)

    def test_a_paid_family_waits_for_the_kimi_route_unless_overridden(self):
        with rig() as r:
            table = json.loads(kill.PROVIDERS.read_text())
            table["routes"]["kimi"]["confirmed"] = False
            kill.PROVIDERS.write_text(json.dumps(table))
            r.module("paid", ["flash"], {"flash": 0.2})
            code, out = quiet(["run", "paid", "--binary", str(r.yi), "--dry"])
            self.assertEqual(code, 2, out)
            self.assertIn("a paid run waits for the Kimi route", out)
            self.assertEqual(quiet(["run", "paid", "--binary", str(r.yi), "--dry", "--without-kimi"])[0], 0)

    def test_a_later_ladder_step_waits_behind_an_earlier_test_module(self):
        with rig() as r:
            r.spend(20.0)
            r.module("first", ["flash"], {"flash": 6.0}, step=1)
            r.module("second", ["flash"], {"flash": 5.0}, step=2)
            code, out = quiet(["run", "second", "--binary", str(r.yi)])
            self.assertEqual((code, out.strip()), (1, "second flash: unrun, needs $5.00"), "20 + 6 waiting + 5 passes 30")

    def test_kimi_runs_when_the_dollar_ladder_is_full(self):
        with rig() as r:
            r.kimi_login(expires_in=10 ** 9)
            r.streams(EVENTS)
            r.spend(25.0)
            r.module("early", ["flash"], {"flash": 10.0}, step=1)
            r.module("late", ["kimi"], {"kimi": 0.5}, step=2)
            code, out = quiet(["run", "late", "--binary", str(r.yi)])
            self.assertEqual((code, len([n for n in r.notes() if "ask" in n])), (0, 1), out)

    def test_a_crashed_kimi_family_is_not_settled_so_the_paid_family_stays_refused(self):
        with rig() as r:
            r.module("crash", ["kimi", "flash"], {"kimi": 0.5, "flash": 0.2})
            code, out = quiet(["run", "crash", "--binary", str(r.yi), "--family", "kimi"])
            self.assertEqual(code, 1, out)
            self.assertIn("crash kimi: 0 runs, $0.0000, 1 not run, needs $0.50:", out)
            code, out = quiet(["run", "crash", "--binary", str(r.yi), "--family", "flash"])
            self.assertEqual(code, 2, out)
            self.assertIn("refused: crash runs kimi before flash", out)
            self.assertEqual([n for n in r.notes() if "ask" in n], [], "no paid run before Kimi ran")


class Quota(unittest.TestCase):
    NOW = 1_800_000_000.0

    def virtual(self):
        now, slept = [self.NOW], []
        kill.clock = lambda: now[0]

        def sleep(seconds):
            slept.append(seconds)
            now[0] += seconds
        kill.sleep = sleep
        return slept

    def usage(self, reset):
        iso = datetime.datetime.fromtimestamp(reset, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        late = datetime.datetime.fromtimestamp(reset + 86400 * 5, datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        os.environ["FAKE_USAGE"] = json.dumps({"limit_5h": {"used": 1.0, "reset": iso},
                                               "limit_7d": {"used": 0.4, "reset": late}})

    def test_an_exhausted_quota_parks_kimi_until_the_reset_then_runs(self):
        with rig() as r:
            slept = self.virtual()
            r.kimi_login(expires_in=10 ** 9)
            r.streams(QUOTA, EVENTS)
            self.usage(self.NOW + 3600)
            row = kill.ask("t", "kimi", r.spec, str(r.yi), float("inf"), until=self.NOW + 7200, estimate_usd=0.5)
            self.assertEqual(row["status"], "ok", row)
            self.assertEqual(slept, [3600.0], "the retry must wait for the 5-hour window's reset")
            self.assertGreaterEqual(row["startedAt"], self.NOW + 3600, row)

    def test_a_reset_past_the_deadline_writes_the_run_unrun_with_its_estimate(self):
        with rig() as r:
            self.virtual()
            r.kimi_login(expires_in=10 ** 9)
            r.streams(QUOTA, EVENTS)
            self.usage(self.NOW + 3600)
            row = kill.ask("t", "kimi", r.spec, str(r.yi), float("inf"), until=self.NOW + 1800, estimate_usd=0.5)
            self.assertEqual((row["status"], row["needsUsd"]), ("unrun", 0.5), row)
            self.assertEqual(len([n for n in r.notes() if "ask" in n]), 1, "the exhausted route must not be asked again")

    def test_a_family_parked_past_its_deadline_exits_nonzero_and_reports_unrun(self):
        with rig() as r:
            self.virtual()
            r.kimi_login(expires_in=10 ** 9)
            r.streams(QUOTA, EVENTS)
            self.usage(self.NOW + 3600)
            r.module("parked", ["kimi", "flash"], {"kimi": 0.5, "flash": 0.2})
            until = datetime.datetime.fromtimestamp(self.NOW + 1800, datetime.timezone.utc).isoformat()
            code, out = quiet(["run", "parked", "--binary", str(r.yi), "--until", until, "--family", "kimi"])
            self.assertEqual(code, 1, out)
            self.assertIn("parked kimi: 0 runs, $0.0000, 1 not run, needs $0.50", out)
            self.assertEqual(quiet(["report"])[1].strip(), "parked: unrun: needs $0.70")
            code, out = quiet(["run", "parked", "--binary", str(r.yi), "--family", "flash"])
            self.assertEqual((code, out.strip()), (2, "refused: parked runs kimi before flash"), "Kimi never ran")

    def test_a_quota_that_never_resets_is_not_run_and_never_judged(self):
        with rig() as r:
            r.kimi_login(expires_in=10 ** 9)
            r.streams(QUOTA)
            os.environ["FAKE_USAGE"] = "{}"
            r.module("dead", ["kimi"], {"kimi": 0.5})
            code, out = quiet(["run", "dead", "--binary", str(r.yi)])
            self.assertEqual((code, out.strip()), (1, "dead kimi: 0 runs, $0.0000, 1 not run, needs $0.50: quota "
                                                      "exhausted and `yi catalog usage kimi` gave no reset"))
            self.assertEqual(quiet(["report"])[1].strip(), "dead: unrun: needs $0.50")

    def test_a_paid_family_that_meets_a_quota_429_is_not_run_and_never_judged(self):
        with rig() as r:
            r.streams(QUOTA, EVENTS)
            r.module("paidq", ["flash"], {"flash": 0.2})
            code, out = quiet(["run", "paidq", "--binary", str(r.yi)])
            self.assertEqual((code, out.strip()), (1, "paidq flash: 0 runs, $0.0000, 1 not run, "
                                                      "needs $0.20: the quota stayed exhausted"))
            self.assertEqual(len([n for n in r.notes() if "ask" in n]), 1, "a paid route is never parked or retried")
            self.assertEqual(quiet(["report"])[1].strip(), "paidq: unrun: needs $0.20")


class Isolation(unittest.TestCase):
    def test_a_route_s_routing_reaches_the_run_home_config_verbatim(self):
        with rig() as r:
            r.streams(EVENTS)
            kill.ask("t", "flash", r.spec, str(r.yi), 1.0)
            configs = [n["config"] for n in r.notes() if "ask" in n]
            self.assertEqual(configs, [{"telemetry": {"enabled": True}, "routing": {"ignore": ["inference-net"]}}])

    def test_two_runs_at_once_get_cloned_venvs_their_own_clones_and_no_refresh_token(self):
        with rig() as r:
            r.kimi_login(expires_in=60)
            r.streams(EVENTS, pace=0.05)
            rows, gate = [], threading.Barrier(2)

            def one():
                gate.wait()
                rows.append(kill.ask("iso", "kimi", r.spec, str(r.yi), float("inf")))
            threads = [threading.Thread(target=one) for _ in range(2)]
            for thread in threads:
                thread.start()
            for thread in threads:
                thread.join()
            notes = r.notes()
            asks = [n for n in notes if "ask" in n]
            self.assertEqual([n["build"] for n in notes if "build" in n], ["doctor"], "one venv build, the seed's")
            self.assertEqual(len(asks), 2, notes)
            self.assertEqual(len({n["home"] for n in asks}), 2, "each run gets its own HOME")
            self.assertTrue(all(n["venvs"] == ["kernel-venv-fake"] for n in asks), asks)
            self.assertTrue(all(n["git"] and Path(n["ask"]).is_relative_to(r.root / "iso") for n in asks), asks)
            self.assertTrue(all(n["token"] == {"version": 1, "kind": "oauth", "access": "fresh-access",
                                               "expires_ms": n["token"]["expires_ms"]} for n in asks), asks)
            self.assertTrue(all(n["mode"] == "0o600" for n in asks), asks)
            refreshes = [n["refresh"] for n in notes if "refresh" in n]
            self.assertEqual(refreshes, [["login", "kimi", "--refresh", "660"]] * 2, "both runs ask Yi for a fresh-enough token")
            self.assertEqual(len([n for n in notes if "rotated" in n]), 1, "one central refresh, under Yi's lock")
            self.assertTrue(all(row["status"] == "ok" and not row["venvRebuilt"] for row in rows), rows)
            for n in asks:
                self.assertFalse(Path(n["home"]).exists() or Path(n["ask"]).exists(), "settled runs leave nothing")
            central = json.loads((kill.kimi_home() / ".yi" / "providers" / "tokens" / "kimi.json").read_text())
            self.assertEqual(central["refresh"], "rotated", "the rotated refresh token stays central")


class Runs(unittest.TestCase):
    def test_an_empty_seed_boots_its_own_venv_and_the_row_says_so_with_the_stderr_tail(self):
        with rig() as r:
            os.environ["FAKE_NO_VENV"] = "1"
            r.streams(EVENTS)
            row = kill.ask("t", "flash", r.spec, str(r.yi), 1.0)
            self.assertTrue(row["venvRebuilt"] is True, row)
            self.assertEqual((len(row["stderr"]), row["stderr"][-9:]), (2000, "the tail\n"), row)

    def test_a_run_past_its_deadline_is_killed_as_a_timeout(self):
        with rig() as r:
            kill.GRACE_SEC = 0.3
            r.streams(EVENTS, pace=0.2)
            row = kill.ask("t", "flash", {**r.spec, "deadline": 0}, str(r.yi), 1.0)
            self.assertEqual(row["status"], "timeout", row)
            self.assertLess(row["wallSec"], 5, row)

    def test_a_harbor_sweep_gets_the_reserve_as_its_cap_and_is_charged_its_filed_rows(self):
        with rig() as r:
            kill.TRIALS.mkdir()
            sweep = [sys.executable, "-c", (
                "import json, os, sys\n"
                "row = {'costUsd': 0.12, 'cap': os.environ['TBV4_HARD_CAP'], 'job': sys.argv[2]}\n"
                "open(os.path.join(sys.argv[1], os.environ['EVAL_RUN_ID'] + '.jsonl'), 'a').write(json.dumps(row) + '\\n')\n"),
                str(kill.TRIALS)]
            row = kill.harbor("tb", "flash", {"id": "one", "harbor": [*sweep, "first"]}, 1.0)
            filed = json.loads((kill.TRIALS / "ns-tb-flash-one.jsonl").read_text())
            self.assertEqual((row["status"], row["chargedUsd"], filed["cap"], row["jobs"]), ("ok", 0.12, "1.00", ["first"]), row)
            again = kill.harbor("tb", "flash", {"id": "one", "harbor": [*sweep, "again"]}, 1.0)
            self.assertEqual((again["chargedUsd"], again["trials"], again["jobs"]), (0.12, 1, ["again"]),
                             "a re-run pays for and names only its own rows")
            silent = kill.harbor("tb", "flash", {"id": "two", "harbor": [sys.executable, "-c", "pass"]}, 0.7)
            self.assertEqual(silent["chargedUsd"], 0.7, "a sweep that filed nothing is charged its cap")
            self.assertEqual(kill.harbor("tb", "kimi", {"id": "three", "harbor": sweep}, float("inf"))["status"], "unrun")

    def test_an_old_corrupt_line_is_skipped_and_a_torn_one_this_sweep_filed_charges_the_cap(self):
        with rig() as r:
            kill.TRIALS.mkdir()
            (kill.TRIALS / "ns-tb-flash-one.jsonl").write_text('{"costUsd": 0.5}\n{"costUsd": 0.1\n[1]\n')
            sweep = [sys.executable, "-c", (
                "import json, os, sys\n"
                "open(os.path.join(sys.argv[1], os.environ['EVAL_RUN_ID'] + '.jsonl'), 'a').write("
                "json.dumps({'costUsd': 0.12}) + '\\n')\n"), str(kill.TRIALS)]
            row = kill.harbor("tb", "flash", {"id": "one", "harbor": sweep}, 1.0)
            self.assertEqual((row["status"], row["trials"], row["chargedUsd"]), ("ok", 1, 0.12), row)
            torn = [sys.executable, "-c", (
                "import os, sys\n"
                "open(os.path.join(sys.argv[1], os.environ['EVAL_RUN_ID'] + '.jsonl'), 'a').write("
                "'{\"costUsd\": 0.2}\\n{\"costUsd\": 0.3')\n"), str(kill.TRIALS)]
            row = kill.harbor("tb", "flash", {"id": "one", "harbor": torn}, 0.8)
            self.assertEqual((row["trials"], row["unreadable"], row["chargedUsd"]), (1, 1, 0.8),
                             "a torn final append is charged the cap and named, never dropped")
            self.assertIn("1 line(s) this sweep filed", row["why"])
            fused = [sys.executable, "-c", (
                "import os, sys\n"
                "open(os.path.join(sys.argv[1], os.environ['EVAL_RUN_ID'] + '.jsonl'), 'a').write("
                "'{\"costUsd\": 0.4}\\n{\"costUsd\": 0.1}\\n')\n"), str(kill.TRIALS)]
            row = kill.harbor("tb", "flash", {"id": "one", "harbor": fused}, 0.9)
            self.assertEqual((row["trials"], "unreadable" in row, row["chargedUsd"]), (2, False, 0.5),
                             "a re-run's first row never fuses onto the torn line the last sweep left")

    def test_the_sweep_cap_is_the_lower_of_the_trials_cap_and_the_reserve_handed_down(self):
        with tempfile.TemporaryDirectory() as store:
            own, _ = trials.caps("ns-x", 1, store=store, now=0)
            self.assertEqual(trials.caps("ns-x", 1, store=store, now=0, at_most=0.35), (0.35, None))
            self.assertEqual(trials.caps("ns-x", 1, store=store, now=0, at_most=own + 5), (own, None))


class Report(unittest.TestCase):
    def test_every_module_is_listed_with_its_verdict_or_the_dollars_it_needs(self):
        with rig() as r:
            r.module("ran", ["flash"], {"flash": 0.2}, step=1)
            r.module("blocked", ["flash"], {"flash": 0.3}, step=2)
            r.module("never", ["flash"], {"flash": 0.4}, step=3)
            r.module("half", ["kimi", "flash"], {"kimi": 1.0, "flash": 2.0}, step=4)
            r.module("parked", ["kimi"], {"kimi": 0.5}, step=5)
            with kill.ledger() as (_, write):
                write({"kind": "settle", "key": "k", "test": "ran", "family": "flash", "usd": 0.1,
                       "rows": [{"status": "ok"}]})
                write({"kind": "unrun", "test": "blocked", "family": "flash", "needsUsd": 0.3, "why": "x"})
                write({"kind": "settle", "key": "h", "test": "half", "family": "kimi", "usd": 0, "rows": [{"status": "ok"}]})
                write({"kind": "unrun", "test": "half", "family": "flash", "needsUsd": 2.0, "why": "x"})
                write({"kind": "settle", "key": "p", "test": "parked", "family": "kimi", "usd": 0,
                       "rows": [{"status": "unrun", "needsUsd": 0.5, "why": "x"}]})
            code, out = quiet(["report"])
            self.assertEqual(code, 0, out)
            self.assertEqual(out.splitlines(), ["ran: KEEP", "blocked: unrun: needs $0.30", "never: unrun: needs $0.40",
                                                "half: KEEP (unrun: flash needs $2.00)", "parked: unrun: needs $0.50"])


if __name__ == "__main__":
    unittest.main()
