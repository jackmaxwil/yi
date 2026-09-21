"""The levers gates on synthetic rows: no model, no network, no paid run."""
import pathlib, sys, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import levers  # noqa: E402

FLOORS = {"code": {"pass_min": 0.5, "reward_min": 0.5, "tolerance": 0.1, "tasks": ["alpha", "beta"]},
          "data": {"pass_min": 1.0, "reward_min": 1.0, "tolerance": 0.0, "tasks": ["gamma"]}}
SPLIT = {"development": ["alpha", "beta"], "validation": ["gamma"], "final": ["delta"]}


def rows(rewards, cost=1.0, tokens=1000, wall=10.0):
    return [{"task": task, "reward": reward, "input": tokens, "cacheRead": 0, "output": 0,
             "costUsd": cost, "wallSec": wall} for task, reward in rewards.items()]


def measured(rewards, **spend):
    return levers.measure(rows(rewards, **spend), FLOORS)


class Gates(unittest.TestCase):
    def test_a_cheaper_candidate_below_the_floor_is_rejected_with_its_class(self):
        baseline = measured({"alpha": 1, "beta": 1, "gamma": 1})
        cheap = measured({"alpha": 1, "beta": 1, "gamma": 0}, cost=0.1, tokens=100, wall=1.0)
        self.assertEqual(levers.gate(baseline, cheap, FLOORS), "below_floor:data")
        within = measured({"alpha": 1, "beta": 0, "gamma": 1}, cost=0.1)
        self.assertIsNone(levers.gate(baseline, within, FLOORS), "0.5 meets the code floor")
        self.assertEqual(levers.gate(baseline, measured({"alpha": 1, "beta": 1, "gamma": 1}), FLOORS),
                         "no_efficiency_gain")
        self.assertEqual(levers.gate(baseline, measured({"gamma": 1}, cost=0.1), FLOORS), "class_not_run:code")

    def test_a_dominated_candidate_is_not_a_survivor(self):
        field = {"cheap": measured({"alpha": 1, "beta": 1, "gamma": 1}, cost=0.5),
                 "dear": measured({"alpha": 1, "beta": 1, "gamma": 1}, cost=0.9),
                 "fast": measured({"alpha": 1, "beta": 1, "gamma": 1}, cost=0.9, wall=2.0),
                 "twin": measured({"alpha": 1, "beta": 1, "gamma": 1}, cost=0.5)}
        self.assertEqual(levers.survivors(field), ["cheap", "fast", "twin"])

    def test_fit_refuses_rows_that_name_a_held_out_task(self):
        self.assertEqual(len(levers.fit_rows(rows({"alpha": 1, "beta": 0}), SPLIT)), 2)
        for held in ("gamma", "delta"):
            with self.assertRaisesRegex(levers.Refused, f"row 2 names the held-out task '{held}'"):
                levers.fit_rows(rows({"alpha": 1, held: 1}), SPLIT)
        with self.assertRaisesRegex(levers.Refused, "gamma"):
            levers.fit_rows([{"task": "alpha", "note": "tuned against gamma."}], SPLIT)
        self.assertEqual(len(levers.fit_rows([{"task": "alpha", "note": "gammaray"}], SPLIT)), 1)

    def test_a_task_no_floor_covers_is_refused(self):
        with self.assertRaisesRegex(levers.Refused, "'omega' has no class"):
            measured({"alpha": 1, "omega": 1})

    def test_the_shipped_manifest_passes_its_selfcheck(self):
        self.assertEqual(levers.selfcheck(), [])


class Search(unittest.TestCase):
    """A synthetic runner: a run under `knob` spends `saves(repetition)` fewer tokens a task."""

    def runner(self, saves, reward=1, knob="plan.width_max"):
        self.calls = []

        def run(overrides, tasks):
            self.calls.append(dict(overrides))
            turn = sum(1 for call in self.calls if call == overrides) - 1
            moved = overrides.get(knob, 8) != 8
            spent = 1000 - (saves(turn) if moved else 0)
            return rows({task: reward if moved else 1 for task in tasks}, tokens=spent,
                        cost=spent / 1000, wall=spent / 100)
        return run

    def compare(self, run, value=4, tasks=("alpha", "beta", "gamma"), k=3, lever="plan.width_max"):
        return levers.compare(lever, value, list(tasks), run, FLOORS, levers.manifest(), k=k)

    def test_a_comparison_reports_its_interval(self):
        result = self.compare(self.runner(lambda turn: 100 + 10 * turn))
        point, = result["points"]
        tokens = point["intervals"]["tokens"]
        self.assertEqual((point["verdict"], point["reason"], point["levers"]), ("better", None, {"plan.width_max": 4}))
        self.assertEqual((tokens["pairs"], tokens["low"], tokens["median"], tokens["high"]), (9, -120, -110, -100))
        self.assertGreaterEqual(tokens["confidence"], 0.95)
        self.assertEqual(result["spend"], {"runs": 6, "trials": 18})
        self.assertEqual(self.calls, [{}, {"plan.width_max": 4}, {"plan.width_max": 4}, {}, {}, {"plan.width_max": 4}])
        # Two pairs that both saved tokens are a point estimate, not a win: the interval covers 0.5.
        thin, = self.compare(self.runner(lambda turn: 100), tasks=("alpha", "beta"), k=1)["points"]
        self.assertEqual((thin["verdict"], thin["intervals"]["tokens"]["confidence"]), ("inconclusive", 0.5))
        noisy, = self.compare(self.runner(lambda turn: (300, -100, -150)[turn]))["points"]
        self.assertEqual(noisy["verdict"], "inconclusive")
        self.assertLess(noisy["intervals"]["tokens"]["low"], 0)
        self.assertGreater(noisy["intervals"]["tokens"]["high"], 0)
        failing, = self.compare(self.runner(lambda turn: 500, reward=0))["points"]
        self.assertEqual((failing["verdict"], failing["reason"]), ("rejected", "below_floor:code"))

    def test_a_grid_refuses_a_knob_marked_not_tunable(self):
        run, listed = self.runner(lambda turn: 100), levers.manifest()
        with self.assertRaisesRegex(levers.Refused, "plan.spawn_cap is not tunable: a fuse"):
            levers.grid({"plan.width_max": [4], "plan.spawn_cap": [32]}, ["alpha"], run, FLOORS, listed)
        with self.assertRaisesRegex(levers.Refused, "is 15 runs; the bound is 12"):
            levers.grid({"plan.width_max": [2, 4], "todo.nudge_work": [8, 16]}, ["alpha"], run, FLOORS, listed, max_runs=12)
        with self.assertRaisesRegex(levers.Refused, "1 to 5 knobs"):
            levers.grid({name: [row["min"]] for name, row in list(listed.items())[:6]}, ["alpha"], run, FLOORS, listed)
        with self.assertRaisesRegex(levers.Refused, "'omega' has no class"):
            levers.grid({"plan.width_max": [4]}, ["omega"], run, FLOORS, listed)
        self.assertEqual(self.calls, [], "a refused grid spends nothing")
        result = levers.grid({"plan.width_max": [4, 8], "todo.nudge_work": [12, 16]}, ["alpha", "beta", "gamma"],
                             run, FLOORS, listed, k=2)
        self.assertEqual(len(result["points"]), 3, "the point that is all defaults is the baseline, not a candidate")
        self.assertEqual((result["spend"]["runs"], len(self.calls)), (8, 8))
        self.assertEqual(len(result["survivors"]), 2, "the two points that move the knob tie; the third saved nothing")

    def test_a_candidate_outside_its_range_is_refused(self):
        run = self.runner(lambda turn: 100)
        for value, lever in ((17, "plan.width_max"), (0, "plan.width_max"), (True, "plan.width_max"),
                             (4.5, "plan.width_max"), (4, "plan.width_maxx")):
            with self.assertRaisesRegex(levers.Refused, "integer in 1..16|unknown lever"):
                self.compare(run, value=value, lever=lever)
        with self.assertRaisesRegex(levers.Refused, "every point is the defaults"):
            self.compare(run, value=8)
        self.assertEqual(self.calls, [])


if __name__ == "__main__":
    unittest.main()
