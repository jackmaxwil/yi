"""The checker screen's variant builder and scorer on a toy task, and EXP-B3d's verdict at its two
bars: no model, no Docker, no TB task text (benchmark files never enter the tree)."""
import importlib.util, json, os, subprocess, sys, tempfile, unittest
from pathlib import Path
from unittest import mock

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))
import checker_screen  # noqa: E402

SPEC = importlib.util.spec_from_file_location("kill_x_b3d", EVALS / "kill" / "x-b3d.py")
B3D = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(B3D)

BUGGY = "def add(a, b):\n    return a - b\n\n\ndef neg(a):\n    return a\n\n\ndef half(a):\n    return a\n"
FIXED = "def add(a, b):\n    return a + b\n\n\ndef neg(a):\n    return -a\n\n\ndef half(a):\n    return a / 2\n"
# `both` catches two defects, `add` and `neg` one each, `loads` none, `wrong` fails the oracle;
# nothing catches `half`, and four checks pass the oracle against three planted defects.
CHECKS = ("import calc\n"
          "print('PASS both' if calc.add(2, 3) == 5 and calc.neg(2) == -2 else 'FAIL both')\n"
          "print('PASS add' if calc.add(2, 3) == 5 else 'FAIL add')\n"
          "print('PASS neg' if calc.neg(2) == -2 else 'FAIL neg')\n"
          "print('PASS wrong' if calc.neg(2) == 2 else 'FAIL wrong')\n"
          "print('PASS loads')\n")
TOY = {"cwd": "/app", "files": {"calc.py": ("environment/calc.py", "solution/calc.py"),
                                "helper.py": ("environment/helper.py", "solution/helper.py")},
       "defects": {"add": ["calc.py#0"], "neg": ["calc.py#1"], "half": ["calc.py#2"]}, "neutral": ["helper.py"]}
AT = [(3, 3, 0.5), (3, 3, 0.5), (2, 2, 1.0), (2, 1, 0.0)]


def toy(tmp, checks=CHECKS):
    root = Path(tmp) / "toy"
    for side, text in (("environment", BUGGY), ("solution", FIXED)):
        (root / side).mkdir(parents=True)
        (root / side / "calc.py").write_text(text)
    (root / "solution" / "helper.py").write_text("HELP = 1\n")
    (Path(tmp) / "checks.py").write_text(checks)
    return root, Path(tmp) / "checks.py"


def local(cwd, staged):
    """The docker runner's contract on the host: the staged tree is the filesystem."""
    tree = staged / "tree" / cwd.lstrip("/")
    tree.mkdir(parents=True, exist_ok=True)
    done = subprocess.run([sys.executable, str(staged / "checks.py")], cwd=tree, text=True, capture_output=True,
                          env={**os.environ, "PYTHONPATH": str(tree)})
    return done.stdout, done.returncode, False, done.stderr


def oracle_only(result):
    """A runner that answers `result` on the oracle tree and runs every variant locally."""
    def run(cwd, staged):
        oracle = (staged / "tree" / cwd.lstrip("/") / "calc.py").read_text() == FIXED
        return result if oracle else local(cwd, staged)
    return run


class Screen(unittest.TestCase):
    def test_precision_is_oracle_passes_over_checks_and_sensitivity_is_defects_caught_over_planted(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, checks = toy(tmp)
            got = checker_screen.score("toy", TOY, root, checks, local)
        self.assertEqual({k: got[k] for k in ("task", "checks", "oraclePassed", "precision", "caught", "planted",
                                              "sensitivity", "oracleExit")},
                         {"task": "toy", "checks": 5, "oraclePassed": 4, "precision": 4 / 5,
                          "caught": {"add": ["add", "both"], "neg": ["both", "neg"], "half": []}, "planted": 3,
                          "sensitivity": 2 / 3, "oracleExit": 0})

    def test_checks_that_crash_or_time_out_on_the_oracle_count_one_failing_check(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, checks = toy(tmp, "print('PASS one')\nraise RuntimeError('unguarded')\n")
            got = checker_screen.score("toy", TOY, root, checks, local)
            self.assertEqual((got["checks"], got["oraclePassed"], got["precision"], got["oracleExit"]), (2, 1, 0.5, 1))
            late = checker_screen.score("toy", TOY, root, checks, oracle_only(("PASS one\n", None, True, "")))
            self.assertEqual((late["precision"], late["timedOut"]), (0.5, ["oracle"]))

    def test_a_docker_fault_is_no_verdict(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, checks = toy(tmp)
            for code in checker_screen.DOCKER_FAULTS:
                with self.assertRaisesRegex(checker_screen.Unscorable, f"toy oracle: exit {code} is a docker fault"):
                    checker_screen.score("toy", TOY, root, checks, oracle_only(("", code, False, "daemon asleep")))

    def test_a_bare_verdict_word_names_no_check_and_a_repeated_id_fails_if_any_line_did(self):
        self.assertEqual(checker_screen.verdicts("PASS\nFAIL x\nPASS y\nPASS x\n"), {"x": False, "y": True})

    def test_a_map_that_leaves_an_op_out_or_names_one_twice_is_refused_and_a_new_file_reverted_is_removed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, checks = toy(tmp)
            refused = {r"ops in no defect: /app/calc\.py#1, /app/calc\.py#2$": {"add": ["calc.py#0"]},
                       r"calc\.py#3: op 3 is past the file's 3$": {**TOY["defects"], "more": ["calc.py#3"]},
                       r"calc\.py#0: op 0 is already in add$": {**TOY["defects"], "neg": ["calc.py#0", "calc.py#1"]}}
            for pattern, defects in refused.items():
                with self.assertRaisesRegex(ValueError, pattern):
                    list(checker_screen.trees({**TOY, "defects": defects}, root))
            with self.assertRaisesRegex(ValueError, r"calc\.py#2: op 2 is already in half$"):
                list(checker_screen.trees({**TOY, "neutral": ["helper.py", "calc.py#2"]}, root))
            spec = {**TOY, "defects": {**TOY["defects"], "helper": ["helper.py"]}, "neutral": []}
            variant = dict(checker_screen.trees(spec, root))["helper"]
            self.assertEqual((variant["/app/helper.py"], variant["/app/calc.py"]), (None, FIXED))
            checker_screen.stage(variant, checks, Path(tmp) / "staged")
            self.assertEqual((Path(tmp) / "staged" / "remove").read_text(), "/app/helper.py\n")
            self.assertFalse((Path(tmp) / "staged" / "tree" / "app" / "helper.py").exists())


class Verdict(unittest.TestCase):
    def rows(self, family, tasks, status="ok"):
        """One run row per task: (checks, passed, sensitivity) caches a score, None keeps nothing."""
        out = []
        for task, figures in zip(checker_screen.TASKS, tasks):
            run = Path(os.environ["EVAL_KILL_ROOT"]) / "x-b3d" / f"{family}-{task}"
            run.mkdir(parents=True, exist_ok=True)
            if figures:
                checks, passed, sensitivity = figures
                (run / "score.json").write_text(json.dumps(
                    {"task": task, "checks": checks, "oraclePassed": passed, "sensitivity": sensitivity}))
            out.append({"family": family, "id": task, "run": run.name, "status": status})
        return out

    def verdict(self, build):
        with tempfile.TemporaryDirectory() as tmp, mock.patch.dict(os.environ, {"EVAL_KILL_ROOT": tmp}):
            return B3D.verdict(build())

    def test_the_verdict_keeps_at_both_bars_and_kills_one_check_or_one_catch_past_either(self):
        cases = {"at both bars": (AT, "KEEP: "),
                 "one more check failing the oracle": ([*AT[:3], (3, 1, 0.0)], "KILL: "),
                 "median sensitivity under half": ([AT[0], (3, 3, 0.4), *AT[2:]], "KILL: "),
                 "a settled run that kept no checks caught nothing": ([AT[0], (3, 3, 0.4), AT[2], None], "KILL: "),
                 "a task not run is open": (AT[:3], "UNRESOLVED: "),
                 "under half only while the open task counts as nothing": ([(3, 3, 0.4)] * 2 + [(3, 3, 1.0)],
                                                                           "UNRESOLVED: "),
                 "under half even with the open task perfect": ([(3, 3, 0.0)] * 3, "KILL: "),
                 "precision under the bar with a task open": ([(9, 1, 1.0)] * 3, "UNRESOLVED: ")}
        for name, (glm, word) in cases.items():
            line = self.verdict(lambda: self.rows("kimi", AT) + self.rows("glm-flash", glm))
            self.assertTrue(line.startswith(word), (name, line))
        line = self.verdict(lambda: self.rows("kimi", AT) + self.rows("glm-flash", AT[:3]))
        self.assertIn("medical-claims-processing 2/2 pass, sensitivity 1.00; cumulative-layout-shift not run", line)
        self.assertTrue(self.verdict(lambda: self.rows("kimi", AT)).startswith("UNRESOLVED: "))

    def test_a_run_cut_short_or_one_docker_could_not_score_is_open_not_a_zero(self):
        cut = self.verdict(lambda: self.rows("kimi", AT) + self.rows("glm-flash", AT[:3])
                           + self.rows("glm-flash", [None] * 4, "budget")[3:])
        self.assertTrue(cut.startswith("UNRESOLVED: ") and "cumulative-layout-shift status budget" in cut, cut)

        def kept():
            out = self.rows("kimi", AT) + self.rows("glm-flash", [*AT[:3], None])
            (Path(os.environ["EVAL_KILL_ROOT"]) / "x-b3d" / out[-1]["run"] / "checks.py").write_text("print('PASS x')\n")
            return out
        with mock.patch.object(checker_screen, "docker_runner", side_effect=FileNotFoundError("docker")):
            line = self.verdict(kept)
        self.assertTrue(line.startswith("UNRESOLVED: ") and "unscored, FileNotFoundError: docker" in line, line)


class Scored(unittest.TestCase):
    def test_a_kept_checks_file_is_scored_once_and_a_harness_fault_or_a_leak_caches_nothing(self):
        with tempfile.TemporaryDirectory() as tmp, \
                mock.patch.dict(os.environ, {"EVAL_KILL_ROOT": tmp, "EVAL_TB_TASKS": tmp}), \
                mock.patch.dict(checker_screen.TASKS, {"toy": TOY}):
            toy(tmp)
            base = Path(tmp) / "x-b3d"
            for run in ("kept", "none", "fault", "leak"):
                (base / run).mkdir(parents=True)
            for run in ("kept", "fault", "leak"):
                (base / run / "checks.py").write_text(CHECKS)
            (base / "leak" / "events.jsonl").write_text('{"cmd": "cat ../../yi/ref/benchmarks/x/solution/a"}\n')
            with mock.patch.object(checker_screen, "docker_runner", return_value=local):
                got = B3D.scored({"id": "toy", "run": "kept"})
                self.assertEqual((got["precision"], got["sensitivity"]), (4 / 5, 2 / 3))
                self.assertEqual(B3D.scored({"id": "toy", "run": "none"})["sensitivity"], 0.0)
                with self.assertRaisesRegex(checker_screen.Unscorable, "ref/benchmarks"):
                    B3D.scored({"id": "toy", "run": "leak"})
            with mock.patch.object(checker_screen, "docker_runner", return_value=oracle_only(("", 125, False, ""))):
                with self.assertRaises(checker_screen.Unscorable):
                    B3D.scored({"id": "toy", "run": "fault"})
            with mock.patch.object(checker_screen, "docker_runner", side_effect=AssertionError("re-scored")):
                self.assertEqual(B3D.scored({"id": "toy", "run": "kept"}), got)
            self.assertFalse((base / "fault" / "score.json").exists() or (base / "leak" / "score.json").exists())


if __name__ == "__main__":
    unittest.main()
