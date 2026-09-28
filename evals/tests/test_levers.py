"""The levers gates on synthetic rows: no model, no network, no paid run."""
import contextlib, io, pathlib, sys, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parents[1]))
import levers  # noqa: E402

FLOORS = {"code": {"pass_min": 0.5, "reward_min": 0.5, "tolerance": 0.1, "tasks": ["alpha", "beta"]},
          "data": {"pass_min": 1.0, "reward_min": 1.0, "tolerance": 0.0, "tasks": ["gamma"]},
          "wide": {"pass_min": 0.0, "reward_min": 0.0, "tolerance": 0.0, "tasks": ["w1", "w2", "w3"]}}
SIX = ("alpha", "beta", "gamma", "w1", "w2", "w3")
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

    def test_a_row_that_does_not_order_or_is_missing_is_refused(self):
        baseline = measured({"alpha": 1, "beta": 1, "gamma": 1})
        for broken in (float("nan"), float("inf"), "1.0", True):
            bad = [{"task": "alpha", "reward": broken}]
            with self.assertRaisesRegex(levers.Refused, "which is not a number"):
                levers.measure(bad, FLOORS)
            with self.assertRaisesRegex(levers.Refused, "which is not a number"):
                levers.per_task(bad)
        # A candidate that ran the easy task of a class only would pass the floor by not
        # having been asked the hard one.
        partial = levers.measure(rows({"alpha": 1}, cost=0.1) + rows({"gamma": 1}), FLOORS)
        self.assertEqual(levers.gate(baseline, partial, FLOORS), "task_not_run:beta")

    def test_the_shipped_manifest_passes_its_selfcheck(self):
        self.assertEqual(levers.selfcheck(), [])

    def test_a_lever_that_is_not_tunable_without_a_why_is_named(self):
        def load(name):
            doc = levers.read(name)
            if name == "levers.json":
                for row in doc["levers"]:
                    if row["tunable"] is False:
                        del row["why"]
                        break
            return doc

        found = levers.selfcheck(load)
        self.assertEqual(len(found), 1, found)
        self.assertIn("a lever that is not tunable says why", found[0])


class Search(unittest.TestCase):
    """A synthetic runner: a run under `knob` spends `saves(repetition, task)` fewer tokens."""

    def runner(self, saves, reward=1, knob="plan.width_max"):
        self.calls = []

        def run(overrides, tasks):
            self.calls.append(dict(overrides))
            turn = sum(1 for call in self.calls if call == overrides) - 1
            moved = overrides.get(knob, 8) != 8
            out = []
            for task in tasks:
                spent = 1000 - (saves(turn, task) if moved else 0)
                out += rows({task: reward if moved else 1}, tokens=spent, cost=spent / 1000, wall=spent / 100)
            return out
        return run

    def compare(self, run, value=4, tasks=SIX, k=3, lever="plan.width_max"):
        return levers.compare(lever, value, list(tasks), run, FLOORS, levers.manifest(), k=k)

    def test_a_comparison_reports_its_interval(self):
        result = self.compare(self.runner(lambda turn, task: 100 + 10 * turn + SIX.index(task)))
        point, = result["points"]
        tokens = point["intervals"]["tokens"]
        self.assertEqual((point["verdict"], point["reason"], point["road"], point["levers"]),
                         ("better", None, "economy", {"plan.width_max": 4}))
        self.assertEqual((tokens["pairs"], tokens["low"], tokens["high"]), (6, -115, -110),
                         "one difference per task: the mean over its three repetitions")
        self.assertGreaterEqual(tokens["confidence"], 0.95)
        self.assertEqual(result["spend"], {"runs": 6, "trials": 36})
        self.assertEqual(self.calls, [{}, {"plan.width_max": 4}, {"plan.width_max": 4}, {}, {}, {"plan.width_max": 4}])
        self.assertEqual(sorted(point["per_task"]["tokens"]), sorted(SIX), "the saving is reported per task")
        # Two tasks that both saved tokens are a point estimate, not a win, however many
        # repetitions ran: the interval covers 0.5, and the run says so before it spends.
        with contextlib.redirect_stderr(io.StringIO()) as said:
            thin, = self.compare(self.runner(lambda turn, task: 100), tasks=("alpha", "beta"), k=3)["points"]
        self.assertEqual((thin["verdict"], thin["intervals"]["tokens"]["confidence"]), ("inconclusive", 0.5))
        self.assertIn(f"an interval needs {levers.MIN_TASKS} tasks to reach 0.95", said.getvalue())
        noisy, = self.compare(self.runner(lambda turn, task: (300, -100, -150, 200, -50, 10)[SIX.index(task)]))["points"]
        self.assertEqual(noisy["verdict"], "inconclusive")
        self.assertLess(noisy["intervals"]["tokens"]["low"], 0)
        self.assertGreater(noisy["intervals"]["tokens"]["high"], 0)
        failing, = self.compare(self.runner(lambda turn, task: 500, reward=0))["points"]
        self.assertEqual((failing["verdict"], failing["reason"]), ("rejected", "below_floor:code"))

    def test_a_grid_refuses_a_knob_marked_not_tunable(self):
        run, listed = self.runner(lambda turn, task: 100), levers.manifest()
        with self.assertRaisesRegex(levers.Refused, "plan.spawn_cap is not tunable: a fuse"):
            levers.grid({"plan.width_max": [4], "plan.spawn_cap": [32]}, ["alpha"], run, FLOORS, listed)
        with self.assertRaisesRegex(levers.Refused, "is 15 runs; the bound is 12"):
            levers.grid({"plan.width_max": [2, 4], "todo.nudge_work": [8, 16]}, ["alpha"], run, FLOORS, listed, max_runs=12)
        with self.assertRaisesRegex(levers.Refused, "1 to 5 knobs"):
            levers.grid({name: [row["min"]] for name, row in list(listed.items())[:6]}, ["alpha"], run, FLOORS, listed)
        with self.assertRaisesRegex(levers.Refused, "'omega' has no class"):
            levers.grid({"plan.width_max": [4]}, ["omega"], run, FLOORS, listed)
        self.assertEqual(self.calls, [], "a refused grid spends nothing")
        result = levers.grid({"plan.width_max": [4, 8], "todo.nudge_work": [12, 16]}, list(SIX),
                             run, FLOORS, listed, k=2)
        self.assertEqual(len(result["points"]), 3, "the point that is all defaults is the baseline, not a candidate")
        self.assertEqual((result["spend"]["runs"], len(self.calls)), (8, 8))
        self.assertEqual(len(result["survivors"]), 2, "the two points that move the knob tie; the third saved nothing")

    def test_a_candidate_outside_its_range_is_refused(self):
        run = self.runner(lambda turn, task: 100)
        for value, lever in ((17, "plan.width_max"), (0, "plan.width_max"), (True, "plan.width_max"),
                             (4.5, "plan.width_max"), (4, "plan.width_maxx")):
            with self.assertRaisesRegex(levers.Refused, "integer in 1..16|unknown lever"):
                self.compare(run, value=value, lever=lever)
        with self.assertRaisesRegex(levers.Refused, "every point is the defaults"):
            self.compare(run, value=8)
        self.assertEqual(self.calls, [])


SLICE = [f"t{i:02}" for i in range(12)]
TASK_FLOORS = {"slice": {"pass_min": 0.0, "reward_min": 0.0, "tolerance": 0.0, "tasks": SLICE}}


def trial(task, graded=0.5, cost=1.0, wall=100.0, tokens=1000, reward=0.0, **extra):
    return {"task": task, "reward": reward, "partialScore": graded, "input": tokens, "cacheRead": 0,
            "output": 0, "costUsd": cost, "wallSec": wall, **extra}


def arms(tasks, k, base, cand):
    """k paired repetitions; `base(task, r)` and `cand(task, r)` return one trial row each."""
    return [[base(t, r) for t in tasks] for r in range(k)], [[cand(t, r) for t in tasks] for r in range(k)]


class TaskLevel(unittest.TestCase):
    """Repetitions of one task are one cluster (0.282.0 limits): one difference per task."""

    def judge(self, baseline, candidate, accesses=1, delta=0.07, tasks=SLICE):
        return levers.judge(baseline, candidate, TASK_FLOORS, accesses=accesses, delta=delta)

    def test_one_task_worse_every_time_is_not_hidden_by_pooling_its_repetitions(self):
        six = SLICE[:6]
        base, cand = arms(six, 3, lambda t, r: trial(t, 0.5),
                          lambda t, r: trial(t, 0.3 if t == "t05" else 0.7, cost=0.8))
        # The pooled 18 pairs put the three losses below rank 5, so the old interval cleared 0.
        pooled = [c["partialScore"] - b["partialScore"] for bb, cc in zip(base, cand) for b, c in zip(bb, cc)]
        self.assertGreater(levers.interval(pooled)["low"], 0)
        got = self.judge(base, cand)
        self.assertEqual(got["intervals"]["graded"]["pairs"], 6, "one difference per task")
        self.assertLess(got["intervals"]["graded"]["low"], 0)
        self.assertNotEqual(got["road"], "capability")

    def test_twelve_tasks_allow_one_non_positive_task_at_four_accesses(self):
        def cand(bad):
            return lambda t, r: trial(t, 0.4 if t in bad else 0.6)
        one = self.judge(*arms(SLICE, 2, lambda t, r: trial(t, 0.5), cand({"t00"})), accesses=4)
        self.assertEqual((one["verdict"], one["road"]), ("better", "capability"))
        self.assertGreaterEqual(one["intervals"]["graded"]["confidence"], 1 - 0.05 / 4)
        two = self.judge(*arms(SLICE, 2, lambda t, r: trial(t, 0.5), cand({"t00", "t01"})), accesses=4)
        self.assertEqual(two["verdict"], "inconclusive")

    def test_the_ctrf_tally_counts_only_where_no_trace_scores_the_task(self):
        # N1: most slice tasks score by ctrf test tally (production-planning 16/20), with no trace.
        self.assertEqual(levers.graded({"task": "t00", "reward": 0.0, "testsPassed": 16, "testsTotal": 20,
                                        "traceScored": False}), 0.8)
        # freight-dispatch-shift and vba-userform-port: a wrapper ctrf test passes beside a trace.
        wrapper = {"task": "t00", "reward": 0.0, "partialScore": None, "testsPassed": 1, "testsTotal": 1,
                   "traceScored": True}
        self.assertEqual(levers.graded(wrapper), 0.0, "a passing wrapper test is not a solved trace")
        self.assertEqual(levers.graded({"task": "t00", "reward": 0.0, "partialScore": 0.5603}), 0.5603)
        self.assertEqual(levers.graded({"task": "t00", "reward": 1.0}), 1.0)

    def test_a_one_sided_stop_is_a_loss_and_a_two_sided_one_is_dropped(self):
        base, cand = arms(SLICE, 2, lambda t, r: trial(t, 0.5),
                          lambda t, r: trial(t, 0.9, censored=(t == "t00")))
        got = self.judge(base, cand)
        self.assertEqual(got["per_task"]["graded"]["t00"], -0.5, "the censored arm scores 0, not 0.9")
        both = lambda t, r: trial(t, 0.5, censored=t in {"t00", "t01", "t02"})
        dropped = self.judge(*arms(SLICE, 2, both, both))
        self.assertEqual((dropped["verdict"], dropped["reason"]), ("inconclusive", "pairs_dropped"))

    def test_a_point_inside_ten_percent_is_not_a_cost_bound(self):
        dear = {"t09", "t10", "t11"}
        base, cand = arms(SLICE, 2, lambda t, r: trial(t, 0.5),
                          lambda t, r: trial(t, 0.7, cost=1.25 if t in dear else 1.0))
        self.assertLess(sum(row["costUsd"] for run in cand for row in run)
                        / sum(row["costUsd"] for run in base for row in run), 1.10)
        got = self.judge(base, cand)
        self.assertGreater(got["intervals"]["relCost"]["high"], 0.10)
        self.assertEqual(got["verdict"], "inconclusive")

    def test_a_cheaper_candidate_that_loses_delta_is_not_non_inferior(self):
        cheap = lambda loss: lambda t, r: trial(t, 0.5 - loss, cost=0.8, wall=80.0, tokens=800)
        self.assertEqual(self.judge(*arms(SLICE, 2, lambda t, r: trial(t, 0.5), cheap(0.07)))["verdict"],
                         "inconclusive")
        better = self.judge(*arms(SLICE, 2, lambda t, r: trial(t, 0.5), cheap(0.03)))
        self.assertEqual((better["verdict"], better["road"]), ("better", "economy"))

    def test_a_lost_pass_and_unpriced_trials_have_their_own_reasons(self):
        base = lambda t, r: trial(t, 0.5, reward=1.0 if t == "t00" else 0.0)
        lost = self.judge(*arms(SLICE, 2, base, lambda t, r: trial(t, 0.9)))
        self.assertEqual((lost["verdict"], lost["reason"]), ("rejected", "pass_lost"))
        unpriced = lambda t, r: trial(t, 0.9, cost=None if t in {"t00", "t01"} and r == 0 else 1.0)
        got = self.judge(*arms(SLICE, 2, lambda t, r: trial(t, 0.5), unpriced))
        self.assertEqual((got["verdict"], got["reason"]), ("inconclusive", "unmeasured"))

    def test_the_dev_screen_reads_the_median_task(self):
        base = lambda t, r: trial(t, 0.5)
        self.assertIsNone(levers.screen(*arms(SLICE[:6], 3, base, lambda t, r: trial(t, 0.5 if t < "t03" else 0.6)),
                                        TASK_FLOORS))
        self.assertEqual(levers.screen(*arms(SLICE[:6], 3, base, lambda t, r: trial(t, 0.4)), TASK_FLOORS),
                         "median_task_worse")


if __name__ == "__main__":
    unittest.main()
