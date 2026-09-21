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


if __name__ == "__main__":
    unittest.main()
