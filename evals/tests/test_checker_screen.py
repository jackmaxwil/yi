"""The checker screen's variant builder and scorer on a toy task, and EXP-B3d's verdict at its two
bars: no model, no Docker, no TB task text (benchmark files never enter the tree)."""
import importlib.util, json, os, subprocess, sys, tempfile, unittest
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))
import checker_screen  # noqa: E402

SPEC = importlib.util.spec_from_file_location("kill_x_b3d", EVALS / "kill" / "x-b3d.py")
B3D = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(B3D)

BUGGY = "def add(a, b):\n    return a - b\n\n\ndef neg(a):\n    return a\n"
FIXED = "def add(a, b):\n    return a + b\n\n\ndef neg(a):\n    return -a\n"
# `add` catches its defect, `wrong` fails the oracle, `loads` passes everywhere and catches nothing.
CHECKS = ("import calc\n"
          "print('PASS add' if calc.add(2, 3) == 5 else 'FAIL add')\n"
          "print('PASS wrong' if calc.neg(2) == 2 else 'FAIL wrong')\n"
          "print('PASS loads')\n")
TOY = {"cwd": "/app", "files": {"calc.py": ("environment/calc.py", "solution/calc.py"),
                                "helper.py": ("environment/helper.py", "solution/helper.py")},
       "defects": {"add": ["calc.py#0"], "neg": ["calc.py#1"]}, "neutral": ["helper.py"]}


def toy(tmp):
    root = Path(tmp) / "task"
    for side, text in (("environment", BUGGY), ("solution", FIXED)):
        (root / side).mkdir(parents=True)
        (root / side / "calc.py").write_text(text)
    (root / "solution" / "helper.py").write_text("HELP = 1\n")
    (Path(tmp) / "checks.py").write_text(CHECKS)
    return root, Path(tmp) / "checks.py"


def local(cwd, staged):
    """The docker runner's contract on the host: the staged tree is the filesystem."""
    tree = staged / "tree" / cwd.lstrip("/")
    tree.mkdir(parents=True, exist_ok=True)
    done = subprocess.run([sys.executable, str(staged / "checks.py")], cwd=tree, text=True, capture_output=True,
                          env={**os.environ, "PYTHONPATH": str(tree)})
    return done.stdout, False


class Screen(unittest.TestCase):
    def test_a_check_set_that_catches_one_of_two_defects_and_fails_the_oracle_once(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, checks = toy(tmp)
            got = checker_screen.score("toy", TOY, root, checks, local)
        self.assertEqual({k: got[k] for k in ("checks", "oraclePassed", "caught", "planted", "sensitivity")},
                         {"checks": 3, "oraclePassed": 2, "caught": {"add": ["add"], "neg": []}, "planted": 2,
                          "sensitivity": 0.5})
        self.assertLess(got["precision"], 1.0)

    def test_a_bare_verdict_word_names_no_check_and_a_repeated_id_fails_if_any_line_did(self):
        self.assertEqual(checker_screen.verdicts("PASS\nFAIL x\nPASS y\nPASS x\n"), {"x": False, "y": True})

    def test_an_op_in_no_defect_is_refused_by_name_and_a_new_file_reverted_is_removed(self):
        with tempfile.TemporaryDirectory() as tmp:
            root, checks = toy(tmp)
            with self.assertRaisesRegex(ValueError, r"ops in no defect: /app/calc\.py#1$"):
                list(checker_screen.trees({**TOY, "defects": {"add": ["calc.py#0"]}}, root))
            with self.assertRaisesRegex(ValueError, r"calc\.py#2: op 2 is past the file's 2"):
                list(checker_screen.trees({**TOY, "defects": {"add": ["calc.py#0", "calc.py#1", "calc.py#2"]}}, root))
            spec = {**TOY, "defects": {**TOY["defects"], "helper": ["helper.py"]}, "neutral": []}
            variant = dict(checker_screen.trees(spec, root))["helper"]
            self.assertEqual((variant["/app/helper.py"], variant["/app/calc.py"]), (None, FIXED))
            checker_screen.stage(variant, checks, Path(tmp) / "staged")
            self.assertEqual((Path(tmp) / "staged" / "remove").read_text(), "/app/helper.py\n")
            self.assertFalse((Path(tmp) / "staged" / "tree" / "app" / "helper.py").exists())


class Verdict(unittest.TestCase):
    def rows(self, root, family, tasks):
        """One run row per (checks, passed, sensitivity) task, its score cached beside the run."""
        out = []
        for task, (checks, passed, sensitivity) in zip(checker_screen.TASKS, tasks):
            run = f"{family}-{task}"
            (root / "x-b3d" / run).mkdir(parents=True)
            (root / "x-b3d" / run / "score.json").write_text(json.dumps(
                {"task": task, "checks": checks, "oraclePassed": passed, "sensitivity": sensitivity}))
            out.append({"family": family, "id": task, "run": run, "status": "ok"})
        return out

    def test_the_verdict_keeps_at_both_bars_and_kills_one_check_or_one_catch_past_either(self):
        at = [(3, 3, 0.5), (3, 3, 0.5), (2, 2, 1.0), (2, 1, 0.0)]
        cases = {"at both bars": (at, "KEEP: "),
                 "one more check failing the oracle": ([*at[:3], (3, 1, 0.0)], "KILL: "),
                 "median sensitivity under half": ([at[0], (3, 3, 0.4), *at[2:]], "KILL: "),
                 "a task with no checks counts as nothing caught": ([at[0], (3, 3, 0.4), at[2]], "KILL: ")}
        saved = os.environ.get("EVAL_KILL_ROOT")
        try:
            for name, (glm, word) in cases.items():
                with tempfile.TemporaryDirectory() as tmp:
                    os.environ["EVAL_KILL_ROOT"] = tmp
                    rows = self.rows(Path(tmp), "kimi", at) + self.rows(Path(tmp), "glm-flash", glm)
                    self.assertTrue(B3D.verdict(rows).startswith(word), (name, B3D.verdict(rows)))
            with tempfile.TemporaryDirectory() as tmp:
                os.environ["EVAL_KILL_ROOT"] = tmp
                self.assertTrue(B3D.verdict(self.rows(Path(tmp), "kimi", at)).startswith("UNRESOLVED: "))
        finally:
            os.environ.pop("EVAL_KILL_ROOT") if saved is None else os.environ.update(EVAL_KILL_ROOT=saved)


if __name__ == "__main__":
    unittest.main()
