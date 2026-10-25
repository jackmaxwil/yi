"""EXP-S5-E2's score reader, its pre-registered verdict at each limit and its arm wiring: no model,
no Docker, no Terminal-Bench task text. The verifier files are written by the tests with the keys and
lines the real verifiers print, numbers only (case ids and sentences are task text)."""
import importlib.util, json, os, shutil, sys, tempfile, unittest
from fractions import Fraction
from pathlib import Path
from unittest import mock

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))

SPEC = importlib.util.spec_from_file_location("kill_s5_kc_ab", EVALS / "kill" / "s5-kc-ab.py")
E2 = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(E2)

JOB = EVALS / "fixtures" / "axes" / "harbor" / "job"
# The tally's three sources, as the verifiers write them: heat-pump-warranty and legacy-utility-triage
# a trace_results.json and no ctrf; medical-claims-processing one pytest test that passes whatever
# the agent did, and the line `Scoring: X/Y lines correct` in the verifier's stdout.
TRACE = {"all_or_nothing_reward": 0.0, "partial_score": 0.35, "passed_cases": 7, "total_cases": 20}
CTRF_ONE = {"reportFormat": "CTRF", "results": {"tool": {"name": "pytest"}, "tests": [],
                                                "summary": {"tests": 1, "passed": 1, "failed": 0}}}
STDOUT = "\nScoring: 87/116 lines correct = 75.00%\nCases:   6/10 perfect = 60.00%\nReward:  0.00%\n"


def verifier(trial, **files):
    (trial / "verifier").mkdir(parents=True, exist_ok=True)
    for name, body in files.items():
        (trial / "verifier" / name).write_text(body if isinstance(body, str) else json.dumps(body))


def row(arm, task, passed, units=20, errored=False):
    return {"run": f"ns-s5-kc-ab-kimi-{arm}", "arm": arm, "task": task, "trial": f"{task}__x", "pass": passed == units,
            "passed": passed, "units": units, "source": "trace_results.json", "score": Fraction(passed, units),
            "errored": errored, "timedOut": False, "censored": False, "parentPeakTokens": 0, "children": 0,
            "childInputTokens": 0}


def family(kernel, workers, solo, units=20):
    """{arm: rows} from nine passed counts per arm, task-major: the three repeats of the first task first."""
    return {arm: [row(arm, E2.TASKS[i // E2.REPEATS], n, units) for i, n in enumerate(counts)]
            for arm, counts in (("kernel", kernel), ("workers", workers), ("solo", solo))}


def word(tables):
    return E2.decide(tables)[0]


class Tally(unittest.TestCase):
    def test_a_verifier_is_read_by_its_own_count_not_by_a_one_test_ctrf(self):
        # Failure seen by a consumer: medical-claims-processing's ctrf says 1/1 for every trial, so
        # a ctrf-first reader scores every one of its trials a perfect 1.0 and the task adds noise.
        with tempfile.TemporaryDirectory() as tmp:
            traced, lines, ctrf_only, bare = (Path(tmp) / n for n in ("traced", "lines", "ctrf", "bare"))
            verifier(traced, **{"trace_results.json": TRACE, "ctrf.json": CTRF_ONE})
            verifier(lines, **{"test-stdout.txt": STDOUT, "ctrf.json": CTRF_ONE})
            verifier(ctrf_only, **{"ctrf.json": {"results": {"summary": {"tests": 14, "passed": 12}}}})
            bare.mkdir()
            self.assertEqual(E2.tally(traced), (7, 20, "trace_results.json"))
            self.assertEqual(E2.tally(lines), (87, 116, "test-stdout.txt"))
            self.assertEqual(E2.tally(ctrf_only), (12, 14, "ctrf.json"))
            self.assertIsNone(E2.tally(bare), "a trial whose verifier wrote nothing is unscored, not 0 of 0")


class Judge(unittest.TestCase):
    def test_build_at_a_mean_of_exactly_one_fifth_and_kill_one_step_under(self):
        # Failure: a float mean of nine 0.2s reads 0.19999999999999998 and a true limit case is killed,
        # or a 0.19 case is built and S5-kc-worker-kernel is written on noise.
        self.assertEqual(word({"kimi": family([14] * 9, [10] * 9, [5] * 9),
                               "glm-flash": family([12] * 9, [10] * 9, [5] * 9)}), "BUILD")
        under = [14] * 8 + [13]
        self.assertEqual(word({"kimi": family(under, [10] * 9, [5] * 9)}), "KILL")

    def test_the_lower_bound_must_clear_zero_even_at_a_big_enough_mean(self):
        # mean(d) is 0.2 exactly and SD(d) is 0.33, under the 0.35 bar: 0.2 - 1.8595 * 0.33 / 3 < 0.
        d = [58, 58, 58, -18, -18, -18, 20, 20, 20]
        got = family([40 + x for x in d], [40] * 9, [10] * 9, units=100)
        self.assertEqual(Fraction(sum(d), 900), Fraction(1, 5))
        self.assertEqual(word({"kimi": got}), "KILL")
        self.assertLess(E2.judge(got)["lb"], 0)

    def test_a_spread_past_035_is_unresolved_whatever_the_mean(self):
        # Five 1.0 wins and four ties: mean 0.56, SD 0.53. A BUILD here would rest on a handful of tasks.
        got = family([10, 10, 10, 10, 10, 0, 0, 0, 0], [0] * 9, [0] * 9, units=10)
        self.assertEqual(word({"kimi": got}), "UNRESOLVED")
        self.assertIn("SD(d) 0.53", E2.decide({"kimi": got})[1])

    def test_a_task_scored_in_fewer_than_ten_steps_leaves_it_open(self):
        # At the limit (10 units) it is judged; one fewer is open and names the task.
        counts = ([7] * 9, [5] * 9, [2] * 9)
        ten, nine = family(*counts, units=10), family(*counts, units=9)
        self.assertEqual(E2.judge(ten)["open"], [])
        self.assertEqual(word({"kimi": nine}), "UNRESOLVED")
        self.assertIn("is scored in only 9 steps", E2.decide({"kimi": nine})[1])

    def test_a_missing_or_errored_repeat_leaves_it_open(self):
        got = family([14] * 9, [10] * 9, [5] * 9)
        got["kernel"][4]["errored"] = True
        self.assertEqual(word({"kimi": got}), "UNRESOLVED")
        self.assertIn("kernel legacy-utility-triage has 2 of 3 scored trials", E2.decide({"kimi": got})[1])
        del got["solo"][:3]
        self.assertIn("solo heat-pump-warranty has 0 of 3 scored trials", E2.decide({"kimi": got})[1])

    def test_fewer_passes_than_workers_or_a_score_under_solo_kills_a_big_mean(self):
        # Kernel children lead by 0.42 on score and pass no trial (19 of 20 steps every time); the
        # workers pass two (all 20 steps) and score 0.4 on the rest.
        fewer = family([19] * 9, [20, 20] + [8] * 7, [5] * 9)
        self.assertGreaterEqual(E2.judge(fewer)["d"], Fraction(1, 5))
        self.assertGreater(E2.judge(fewer)["lb"], 0)
        self.assertEqual(word({"kimi": fewer}), "KILL")
        self.assertEqual(word({"kimi": family([14] * 9, [10] * 9, [15] * 9)}), "KILL")

    def test_flash_is_read_only_once_kimi_has_cleared(self):
        kimi = family([14] * 9, [10] * 9, [5] * 9)
        edge = family([12] * 9, [10] * 9, [5] * 9)
        self.assertEqual(word({"kimi": kimi, "glm-flash": edge}), "BUILD")
        self.assertEqual(word({"kimi": kimi, "glm-flash": family([12] * 8 + [11], [10] * 9, [5] * 9)}), "KILL")
        self.assertEqual(word({"kimi": kimi}), "UNRESOLVED")
        self.assertTrue(E2.decide({"kimi": kimi})[1].endswith("; K3.1 cleared and flash is not settled"))
        killed = family([12] * 9, [10] * 9, [5] * 9)
        self.assertEqual(word({"kimi": killed, "glm-flash": edge}), "KILL", "K3.1 under 0.2 kills whatever flash says")
        self.assertEqual(word({"glm-flash": edge}), "UNRESOLVED", "flash alone cannot build it")
        self.assertTrue(E2.decide({"glm-flash": edge})[1].endswith("; no Kimi trial ran"))


class Trials(unittest.TestCase):
    def test_the_parent_is_split_from_its_children_by_the_session_header(self):
        # A fresh trial dir holding two real session files (the axes fixtures: peak 10,410 and
        # 16,000 tokens); the second is given a parent in its header, as a child session has.
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {"TBV4_RUNS_ROOT": tmp}):
            run = "ns-s5-kc-ab-kimi-kernel"
            trial = Path(tmp) / run / "20261024-1" / "heat-pump-warranty__abc"
            sessions = trial / "agent" / "yi" / "sessions"
            sessions.mkdir(parents=True)
            (trial.parent / "result.json").write_text("{}")
            (trial / "result.json").write_text(json.dumps({"task_name": "terminal-bench/heat-pump-warranty",
                                                           "verifier_result": {"rewards": {"reward": 0.0}}}))
            verifier(trial, **{"trace_results.json": TRACE})
            for name in ("fixture-a__x", "fixture-b__y"):
                for path in (JOB / name / "agent" / "yi" / "sessions").glob("*.jsonl"):
                    lines = path.read_text().splitlines()
                    if name == "fixture-b__y":
                        lines[0] = json.dumps({**json.loads(lines[0]), "parentSessionId": "fixture-a"})
                    (sessions / path.name).write_text("\n".join(lines) + "\n")
            (got,) = E2.trials_of(run)
        self.assertEqual((got["task"], got["arm"], got["pass"], got["score"]),
                         ("heat-pump-warranty", "kernel", False, Fraction(7, 20)))
        self.assertEqual((got["parentPeakTokens"], got["children"], got["childInputTokens"]), (10410, 1, 16000))


    def test_the_verdict_reads_every_arms_run_dir_once_however_many_rows_name_it(self):
        # Failure: the report line says UNRESOLVED for a finished A/B because an arm's dir was missed,
        # or a row filed twice for one run doubles its trials and hides a repeat that never ran.
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {"TBV4_RUNS_ROOT": tmp}):
            rows = []
            for family, kernel in (("kimi", 14), ("glm-flash", 12)):
                for arm, passed in (("kernel", kernel), ("workers", 10), ("solo", 5)):
                    run = f"ns-s5-kc-ab-{family}-{arm}"
                    job = Path(tmp) / run / "20261024-1"
                    job.mkdir(parents=True)
                    (job / "result.json").write_text("{}")
                    for i, task in enumerate(t for t in E2.TASKS for _ in range(E2.REPEATS)):
                        trial = job / f"{task}__{i}"
                        trial.mkdir()
                        (trial / "result.json").write_text(json.dumps({
                            "task_name": f"terminal-bench/{task}", "verifier_result": {"rewards": {"reward": 0.0}}}))
                        verifier(trial, **{"trace_results.json": {"passed_cases": passed, "total_cases": 20}})
                    rows.append({"test": "s5-kc-ab", "family": family, "run": run, "status": "ok"})
            self.assertEqual(E2.verdict(rows[:3]).split(":")[0], "UNRESOLVED")
            full = E2.verdict(rows + rows[:1])
            self.assertTrue(full.startswith("BUILD: kimi d +0.20 (LB +0.20, SD 0.00)"), full)
            self.assertIn("glm-flash d +0.10", full)
            shutil.rmtree(Path(tmp) / "ns-s5-kc-ab-kimi-kernel" / "20261024-1" / "heat-pump-warranty__2")
            short = E2.verdict(rows + rows)
            self.assertTrue(short.startswith("UNRESOLVED: kimi open: kernel heat-pump-warranty has 2 of 3 scored trials"), short)


class Arms(unittest.TestCase):
    def test_the_two_child_arms_differ_in_the_role_and_nothing_else(self):
        # Failure: grouping advice that differs between arms makes the A/B measure the prompt.
        argv = {spec["id"]: spec["harbor"] for spec in E2.runs()}
        self.assertEqual(sorted(argv), sorted(E2.ARMS))
        texts = {}
        for arm in ("workers", "kernel"):
            (path,) = [a.split("=", 1)[1] for a in argv[arm] if a.startswith("EVAL_ARM_FILE=")]
            texts[arm] = Path(path).read_text().splitlines()
        differ = [(w, k) for w, k in zip(texts["workers"], texts["kernel"]) if w != k]
        self.assertEqual(len(texts["workers"]), len(texts["kernel"]))
        self.assertEqual(len(differ), 1, differ)
        self.assertIn('role="worker"', differ[0][0])
        self.assertIn('role="root"', differ[0][1])
        self.assertIn("-u", argv["solo"][:3], "the solo arm must not inherit a stale arm file")
        self.assertFalse([a for a in argv["solo"] if a.startswith("EVAL_ARM_FILE=")])
        for arm in E2.ARMS:
            self.assertIn(f"EVAL_ARM={arm}", argv[arm])
            self.assertIn(f"TBV4_ATTEMPTS={E2.REPEATS}", argv[arm])


if __name__ == "__main__":
    unittest.main()
