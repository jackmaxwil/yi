"""The inner loop's task generators: seeded, deterministic, red untouched, green solved.
No model, no network (docs/plans/2026-09-26-self-improvement-evals.md section 19)."""
import pathlib, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "inner"))
import gen  # noqa: E402


def materialize(task, root):
    for relative, body in task["files"].items():
        target = pathlib.Path(root) / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(body)
    return pathlib.Path(root)


class Generators(unittest.TestCase):
    def test_every_family_is_seeded_red_untouched_and_green_solved(self):
        self.assertEqual(sorted(gen.FAMILIES), ["bugfix", "logs", "reconcile"])
        for name, family in gen.FAMILIES.items():
            for seed in (1, 2, 7):
                with self.subTest(family=name, seed=seed):
                    task = family.make(seed)
                    self.assertEqual(task, family.make(seed), "a seed is one task, every time")
                    self.assertTrue(task["prompt"].strip() and task["files"])
                    self.assertLessEqual(task["timeoutSec"], 300)
                    with tempfile.TemporaryDirectory() as tmp:
                        passed, total = family.check(seed, materialize(task, tmp))
                    self.assertGreater(total, 1, "partial credit needs more than one check")
                    self.assertLess(passed, total, "an untouched workspace must not pass")
                    with tempfile.TemporaryDirectory() as tmp:
                        workspace = materialize(task, tmp)
                        family.solve(seed, workspace)
                        self.assertEqual(family.check(seed, workspace), (total, total), "the reference solves it")
            self.assertNotEqual(family.make(1), family.make(2), "seeds differ")

    def test_the_checker_never_trusts_the_workspace_tests(self):
        family = gen.FAMILIES["bugfix"]
        task = family.make(3)
        with tempfile.TemporaryDirectory() as tmp:
            workspace = materialize(task, tmp)
            # An agent that guts the tests instead of fixing the code scores nothing extra.
            for path in workspace.glob("test_*.py"):
                path.write_text("import unittest\n")
            before = family.check(3, materialize(task, tempfile.mkdtemp()))
            self.assertEqual(family.check(3, workspace), before)

    def test_ids_parse_back(self):
        self.assertEqual(gen.parse("logs:12"), ("logs", 12))
        with self.assertRaises(ValueError):
            gen.parse("nope:1")


if __name__ == "__main__":
    unittest.main()
