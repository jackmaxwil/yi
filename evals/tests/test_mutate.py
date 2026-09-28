"""Bug injection into a real repo (SWE-smith's method): seeded span mutations, kept only when the
repo's own suite turns red, graded by the pristine tests. A synthetic repo stands in for the
vendored ones so this runs offline in CI."""
import pathlib, shutil, sys, tempfile, textwrap, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1] / "inner"))
import gen  # noqa: E402
from gen import mutate  # noqa: E402

LIB = textwrap.dedent('''\
    def clamp(x, lo, hi):
        if x < lo:
            return lo
        if x > hi:
            return hi
        return x


    def total(xs):
        acc = 0
        for x in xs:
            acc = acc + x
        return acc


    def either(a, b):
        return a or b
    ''')
TESTS = textwrap.dedent('''\
    import unittest
    from lib import core


    class Core(unittest.TestCase):
        def test_clamp_low(self):
            self.assertEqual(core.clamp(-5, 0, 3), 0)

        def test_clamp_high(self):
            self.assertEqual(core.clamp(9, 0, 3), 3)

        def test_clamp_mid(self):
            self.assertEqual(core.clamp(2, 0, 3), 2)

        def test_clamp_edge(self):
            self.assertEqual(core.clamp(0, 0, 3), 0)

        def test_total(self):
            self.assertEqual(core.total([1, 2, 3]), 6)

        def test_either(self):
            self.assertEqual(core.either(0, 7), 7)
    ''')
# A second module, so a level can plant two distinct bugs.
STATS = "def span(xs):\n    return max(xs) - min(xs)\n\n\ndef mean(xs):\n    return sum(xs) / len(xs)\n"
STATS_TESTS = textwrap.dedent('''\
    import unittest
    from lib import stats


    class Stats(unittest.TestCase):
        def test_span(self):
            self.assertEqual(stats.span([4, 1, 9]), 8)

        def test_mean(self):
            self.assertEqual(stats.mean([2, 4]), 3)

        def test_mean_one(self):
            self.assertEqual(stats.mean([5]), 5)
    ''')


class Mutate(unittest.TestCase):
    def setUp(self):
        self.root = pathlib.Path(tempfile.mkdtemp())
        repo = self.root / "toy"
        (repo / "lib").mkdir(parents=True)
        (repo / "tests").mkdir()
        (repo / ".git").mkdir()
        (repo / "lib" / "__init__.py").write_text("")
        (repo / "lib" / "core.py").write_text(LIB)
        (repo / "tests" / "__init__.py").write_text("")
        (repo / "tests" / "test_core.py").write_text(TESTS)
        (repo / "lib" / "stats.py").write_text(STATS)
        (repo / "tests" / "test_stats.py").write_text(STATS_TESTS)
        self.saved = dict(mutate.REPOS), mutate.CACHE
        mutate.REPOS.clear()
        mutate.REPOS["toy"] = {"path": repo, "src": "lib", "tests": "tests", "commit": None}
        mutate.CACHE = self.root / "cache"

    def tearDown(self):
        mutate.REPOS.clear()
        mutate.REPOS.update(self.saved[0])
        mutate.CACHE = self.saved[1]
        shutil.rmtree(self.root, ignore_errors=True)

    def test_a_seed_is_one_red_bug_the_pristine_tests_grade(self):
        for seed in (1, 2, 3):
            with self.subTest(seed=seed):
                task = mutate.make(seed, 1)
                self.assertEqual(task, mutate.make(seed, 1), "a seed is one task, every time")
                workspace = gen.materialize(task, self.root / f"w{seed}")
                self.assertFalse((workspace / ".git").exists(), "no history to diff the bug out of")
                repo = mutate.REPOS["toy"]["path"]
                self.assertTrue(any((workspace / f).read_text() != (repo / f).read_text() for f in task["patch"]),
                                "a mutation was applied")
                passed, total = mutate.check(seed, workspace, 1)
                self.assertGreaterEqual(total, 2)
                self.assertLess(passed, total, "an untouched workspace is red")
                mutate.solve(seed, workspace, 1)
                self.assertEqual(mutate.check(seed, workspace, 1), (total, total), "the pristine code is green")
                # Deleting the tests in the workspace gains nothing: the checker brings its own.
                shutil.rmtree(workspace / "tests")
                self.assertEqual(mutate.check(seed, workspace, 1), (total, total))

    def test_a_regression_elsewhere_costs_the_clean_point(self):
        task = mutate.make(1, 1)
        workspace = gen.materialize(task, self.root / "w")
        mutate.solve(1, workspace, 1)
        (workspace / "lib" / "core.py").write_text(LIB.replace("return a or b", "return a and b"))
        passed, total = mutate.check(1, workspace, 1)
        self.assertLess(passed, total)

    def test_level_3_hides_the_tests_it_breaks_and_reports_their_failures(self):
        for seed in (1, 2):
            with self.subTest(seed=seed):
                task = mutate.make(seed, 3)
                plan = mutate._plan(seed, 3)
                self.assertEqual(len(plan["sites"]), 2, "two bugs")
                workspace = gen.materialize(task, self.root / f"h{seed}")
                visible = mutate._suite(workspace, mutate.REPOS["toy"])
                self.assertTrue(visible and set(visible.values()) == {"ok"}, "the suite in the workspace passes")
                for test in plan["f2p"]:
                    self.assertNotIn(test, visible, "a test that catches a bug is hidden")
                    self.assertIn(f"{test}: AssertionError", task["prompt"], "its failure is reported instead")
                passed, total = mutate.check(seed, workspace, 3)
                self.assertLess(passed, total, "the pristine tests still grade it")
                mutate.solve(seed, workspace, 3)
                self.assertEqual(mutate.check(seed, workspace, 3), (total, total))


if __name__ == "__main__":
    unittest.main()
