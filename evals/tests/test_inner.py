"""The inner loop's task generators: seeded, deterministic, red untouched, green solved.
No model, no network (docs/plans/2026-09-26-self-improvement-evals.md section 19)."""
import pathlib, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "inner"))
import gen  # noqa: E402


def materialize(task, root):
    return gen.materialize(task, pathlib.Path(root) / "w")


class Generators(unittest.TestCase):
    def test_every_family_is_seeded_red_untouched_and_green_solved(self):
        self.assertEqual(sorted(gen.FAMILIES), ["bugfix", "logs", "mutate", "reconcile", "revert"])
        for name, family in gen.SYNTHETIC.items():
            for level in gen.LEVELS:
                for seed in (1, 2, 7):
                    with self.subTest(family=name, level=level, seed=seed):
                        task = family.make(seed, level)
                        self.assertEqual(task, family.make(seed, level), "a seed is one task, every time")
                        self.assertTrue(task["prompt"].strip() and task["files"])
                        self.assertLessEqual(task["timeoutSec"], 600)
                        with tempfile.TemporaryDirectory() as tmp:
                            passed, total = family.check(seed, materialize(task, tmp), level)
                        self.assertGreater(total, 1, "partial credit needs more than one check")
                        self.assertLess(passed, total, "an untouched workspace must not pass")
                        with tempfile.TemporaryDirectory() as tmp:
                            workspace = materialize(task, tmp)
                            family.solve(seed, workspace, level)
                            self.assertEqual(family.check(seed, workspace, level), (total, total), "the reference solves it")
                self.assertNotEqual(family.make(1, level), family.make(2, level), "seeds differ")

    def test_each_level_adds_what_level_one_saturated_without(self):
        # Inner A/A, 2026-09-28: level 1 of every family scored full on 12 of its first 13 trials.
        logs, bugfix, reconcile = (gen.FAMILIES[name] for name in ("logs", "bugfix", "reconcile"))
        self.assertEqual(sorted(logs.make(4, 2)["files"]), ["edge.log", "service.log"], "two logs to merge")
        self.assertGreater(len(logs.make(4, 3)["files"]["edge.log"]), len(logs.make(4, 2)["files"]["edge.log"]))
        self.assertIn("+02:00", logs.make(4, 3)["files"]["edge.log"], "level 3 writes a local offset")
        for level in (2, 3):
            shown = bugfix.make(4, level)["files"]["test_toolkit.py"].count("def test_")
            self.assertGreater(bugfix.check(4, pathlib.Path(tempfile.mkdtemp()), level)[1], shown, "hidden tests")
        self.assertIn("rates.csv", reconcile.make(4, 3)["files"], "level 3 converts a currency")

    def test_the_checker_never_trusts_the_workspace_tests(self):
        family = gen.FAMILIES["bugfix"]
        for level in gen.LEVELS:
            task = family.make(3, level)
            with tempfile.TemporaryDirectory() as tmp:
                workspace = materialize(task, tmp)
                # An agent that guts the tests instead of fixing the code scores nothing extra.
                for path in workspace.glob("test_*.py"):
                    path.write_text("import unittest\n")
                before = family.check(3, materialize(task, tempfile.mkdtemp()), level)
                self.assertEqual(family.check(3, workspace, level), before)

    def test_ids_parse_back(self):
        self.assertEqual(gen.parse("logs:12"), ("logs", 12, 1))
        self.assertEqual(gen.parse("logs:12:3"), ("logs", 12, 3))
        with self.assertRaises(ValueError):
            gen.parse("logs:12:9")
        with self.assertRaises(ValueError):
            gen.parse("nope:1")


class RolesArm(unittest.TestCase):
    def test_only_the_roles_arm_is_told_which_model_each_duty_takes(self):
        import stat
        import runner
        prompts = {}
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            stub = tmp / "yi"
            stub.write_text('#!/bin/sh\nfor a; do last="$a"; done\nprintf %s "$last" > "$0.prompt"\n')
            stub.chmod(stub.stat().st_mode | stat.S_IXUSR)
            for roles in (False, True):
                row = runner.one("bugfix:1:1", str(stub), "", tmp / f"keep{roles}", roles)
                prompts[roles] = pathlib.Path(f"{stub}.prompt").read_text()
                self.assertEqual((row["roles"], row["model"]), (roles, runner.MODEL))
        for pin in ("deepseek-v4.1-flash", "glm-5.3-flash"):
            self.assertNotIn(pin, prompts[False])
            self.assertIn(pin, prompts[True])


class Cost(unittest.TestCase):
    def test_a_rerun_under_the_same_keep_dir_is_charged_for_its_own_sessions_only(self):
        import runner
        fixture = pathlib.Path(__file__).resolve().parents[1] / "fixtures" / "session" / "1787544431469_fixture-a.jsonl"
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            stub = tmp / "yi"
            stub.write_text('#!/bin/sh\nwhile [ $# -gt 1 ]; do [ "$1" = --session-dir ] && dir="$2"; shift; done\n'
                            f'mkdir -p "$dir" && cp {fixture} "$dir/$$.jsonl"\n')
            stub.chmod(0o755)
            costs = [runner.one("bugfix:1:1", str(stub), "", tmp / "keep")["costUsd"] for _ in range(2)]
        self.assertEqual(costs[0], costs[1])
        self.assertAlmostEqual(costs[1], 0.007432)

    def test_a_request_that_failed_empty_costs_nothing_and_leaves_the_bill_known(self):
        import runner
        fixture = pathlib.Path(__file__).resolve().parents[1] / "fixtures" / "session" / "1787544431469_fixture-a.jsonl"
        failed = {"kind": "entry", "type": "message", "message": {"role": "assistant", "content": [], "stopReason": "error",
                  "usage": {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0, "unknown": True}}}
        with tempfile.TemporaryDirectory() as tmp:
            tmp = pathlib.Path(tmp)
            session = tmp / "session.jsonl"
            session.write_text(fixture.read_text() + __import__("json").dumps(failed) + "\n")
            stub = tmp / "yi"
            stub.write_text('#!/bin/sh\nwhile [ $# -gt 1 ]; do [ "$1" = --session-dir ] && dir="$2"; shift; done\n'
                            f'mkdir -p "$dir" && cp {session} "$dir/s.jsonl"\n')
            stub.chmod(0o755)
            cost = runner.one("bugfix:1:1", str(stub), "", tmp / "keep")["costUsd"]
        self.assertAlmostEqual(cost, 0.007432)


if __name__ == "__main__":
    unittest.main()
