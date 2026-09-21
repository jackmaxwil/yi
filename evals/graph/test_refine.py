"""The refiner's gates on synthetic rows: no model, no network, no paid run."""
import json, pathlib, sys, tempfile, unittest

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import refine  # noqa: E402

FIXTURES = refine.ROOT / "evals/fixtures/graph"
SPLIT = json.loads(refine.SPLIT.read_text())
CONFIG = {"protocol": "synthetic", "model": "faux/one"}
REWORD, ADD, DROP = (json.loads(line) for line in (FIXTURES / "proposals-sample.jsonl").read_text().splitlines())


def shipped():
    return json.loads(refine.GRAPH.read_text())


class Runner:
    """Every task passes at 1000 tokens, except what a graph with `flaw(graph)` does to `task`."""

    def __init__(self, flaw=lambda graph: False, task="seen-red", reward=0, tokens=1000):
        self.flaw, self.task, self.reward, self.tokens, self.calls = flaw, task, reward, tokens, []

    def __call__(self, graph, tasks):
        self.calls.append((graph["version"], tuple(tasks)))
        hit = self.flaw(graph)
        return [{"task": task, "reward": self.reward if hit and task == self.task else 1,
                 "input": self.tokens if hit and task == self.task else 1000, "cacheRead": 0, "output": 0}
                for task in tasks]


def has(edit):
    return lambda graph: any(refine.key(edge) == refine.key(edit["edge"]) for edge in graph["edges"])


class Refiner(unittest.TestCase):
    def setUp(self):
        self.memory = pathlib.Path(tempfile.mkdtemp()) / "rejected.jsonl"

    def refine(self, edits, run, config=CONFIG, graph=None):
        return refine.refine(graph or shipped(), edits, SPLIT, run, config, self.memory, now=lambda: 7)

    def test_a_graph_edit_that_drops_the_held_out_score_is_rejected_and_remembered(self):
        run = Runner(flaw=lambda graph: not has(DROP)(graph))
        graph, (verdict,) = self.refine([DROP], run)
        self.assertEqual((verdict["verdict"], verdict["reason"]), ("rejected", "held_out_passes_dropped"))
        self.assertEqual(graph, shipped(), "a rejected edit leaves the graph as it was")
        self.assertEqual((verdict["scores"]["base"]["heldOut"]["passed"], verdict["scores"]["heldOut"]["passed"]), (3, 2))
        (row,) = [json.loads(line) for line in self.memory.read_text().splitlines()]
        self.assertEqual(set(row), {"editHash", "baseVersion", "protocol", "model", "reason", "scores", "at"})
        self.assertEqual((row["editHash"], row["baseVersion"], row["at"]), (refine.edit_hash(DROP), 1, 7))
        self.assertEqual(refine.edit_hash(DROP), refine.edit_hash(dict(DROP, rationale="said another way")))

        again = Runner(flaw=lambda graph: not has(DROP)(graph))
        _, (verdict,) = self.refine([DROP], again)
        self.assertEqual((verdict["verdict"], verdict["reason"]), ("skipped", "already_rejected:held_out_passes_dropped"))
        self.assertEqual(again.calls, [], "a remembered rejection is never run again")

        # Another model is another question: the old rejection is evidence, and the edit runs.
        _, (verdict,) = self.refine([DROP], Runner(), dict(CONFIG, model="faux/two"))
        self.assertEqual((verdict["verdict"], verdict["scores"]["earlierRejections"]), ("promoted", 1))

    def test_ties_are_accepted_and_tokens_per_solved_task_may_not_rise(self):
        graph, (verdict,) = self.refine([REWORD], Runner())
        self.assertEqual((verdict["verdict"], graph["version"]), ("promoted", 2))
        self.assertLess(verdict["scores"]["guidanceBytes"], 0)
        self.assertEqual(refine.check(graph, {node["id"] for node in graph["nodes"]}), None)

        # The added line costs tokens on a held-out task and solves nothing more: it has not paid for itself.
        _, (verdict,) = self.refine([ADD], Runner(flaw=has(ADD), reward=1, tokens=1200))
        self.assertEqual((verdict["verdict"], verdict["reason"]), ("rejected", "tokens_per_solved_rose"))
        self.assertGreater(verdict["scores"]["guidanceBytes"], 0)

        # Nothing solved on either side prices no token, so added bytes have paid for nothing.
        def solves_nothing(_graph, tasks):
            return [{"task": task, "reward": 0, "input": 1000, "cacheRead": 0, "output": 0}
                    for task in tasks]

        none = dict(CONFIG, protocol="synthetic-none")
        _, (verdict,) = self.refine([ADD], solves_nothing, none)
        self.assertEqual((verdict["verdict"], verdict["reason"]), ("rejected", "guidance_bytes_unpaid"))
        self.assertIsNone(verdict["scores"]["heldOut"]["tokensPerSolved"])
        # The same zero-zero tie without the bytes is a tie, and a tie is accepted.
        _, (verdict,) = self.refine([DROP], solves_nothing, none)
        self.assertEqual((verdict["verdict"], verdict["scores"]["heldOut"]["passed"]), ("promoted", 0))

        # The development tasks filter first, and a candidate that fails them never sees the held-out ones.
        run = Runner(flaw=has(ADD), task="edit-file")
        _, (verdict,) = self.refine([ADD], run, dict(CONFIG, protocol="synthetic-two"))
        self.assertEqual(verdict["reason"], "fit_passes_dropped")
        self.assertEqual([tasks for version, tasks in run.calls if version == 2], [tuple(SPLIT["development"])])

    def test_a_proposal_naming_a_held_out_task_is_refused(self):
        self.assertFalse(set(SPLIT["development"]) & set(SPLIT["validation"] + SPLIT["final"]))
        proposals = self.memory.with_name("proposals.jsonl")
        for task in SPLIT["validation"] + ["a-final-task"]:
            leaked = dict(ADD, rationale=f"this line is what {task} was missing")
            proposals.write_text(json.dumps(REWORD) + "\n" + json.dumps(leaked) + "\n")
            with self.assertRaisesRegex(refine.Refused, task):
                refine.read_proposals(proposals, dict(SPLIT, final=["a-final-task"]))
        # An id hidden by a JSON escape is still the id; an id inside a longer word is not.
        escaped = json.dumps(dict(ADD, rationale="this is what it wanted, seen-red.")).replace("s", "\\u0073")
        proposals.write_text(escaped + "\n")
        with self.assertRaisesRegex(refine.Refused, "seen-red"):
            refine.read_proposals(proposals, SPLIT)
        for punctuated in ("(seen-red)", "seen-red; the other", 'said "seen-red"', "seen-red -- the slow one"):
            proposals.write_text(json.dumps(dict(ADD, rationale=punctuated)) + "\n")
            with self.assertRaisesRegex(refine.Refused, "seen-red"):
                refine.read_proposals(proposals, SPLIT)
        innocent = dict(ADD, rationale="an unseen-redness in the development rows, or a search-looper")
        proposals.write_text(json.dumps(innocent) + "\n")
        self.assertEqual(refine.read_proposals(proposals, SPLIT), [innocent])

        named = dict(ADD, rationale=f"seen in {SPLIT['development'][0]}")
        proposals.write_text(json.dumps(named) + "\n")
        self.assertEqual(refine.read_proposals(proposals, SPLIT), [named], "a development task may inform a proposal")
        proposals.write_text((json.dumps(REWORD) + "\n") * (refine.MAX_EDITS_PER_RUN + 1))
        with self.assertRaisesRegex(refine.Refused, "at most"):
            refine.read_proposals(proposals, SPLIT)
        self.assertEqual(len(refine.read_proposals(FIXTURES / "proposals-sample.jsonl", SPLIT)), 3)

    def test_a_structurally_invalid_edit_never_reaches_a_run(self):
        def run(_graph, _tasks):
            raise AssertionError("an invalid edit reached a run")

        def add(**over):
            return dict(ADD, edge=dict(ADD["edge"], **over))

        edits = {
            "self_edge": add(to="grep"),
            "unknown_node": add(to="a_verb_nobody_registered"),
            "guidance_size": add(guidance="é" * 81),
            "pitfalls_size": add(pitfalls=["one", "two", "three", "four"]),
            "parse": add(condition="the_model_thinks_so"),
            "edge_exists": dict(DROP, kind="add_edge"),
            "no_such_edge": dict(ADD, kind="reword"),
            "no_change": dict(DROP, kind="reword"),
            "malformed_edit": dict(ADD, kind="rewrite_everything"),
        }
        graph, verdicts = self.refine(list(edits.values()), run)
        self.assertEqual([verdict["reason"] for verdict in verdicts], list(edits))
        self.assertEqual(graph, shipped())
        remembered = [json.loads(line)["reason"] for line in self.memory.read_text().splitlines()]
        self.assertEqual(remembered, list(edits), "the check's name is the recorded reason")

        # A drop can break a rule too: with one road left into rlm.run, dropping it strands rlm.run's own edges.
        roads = [edge for edge in graph["edges"] if edge["to"] == "rlm.run"]
        one_road = dict(graph, edges=[edge for edge in graph["edges"] if edge != roads[0]])
        drop = {"kind": "drop_edge", "edge": roads[1], "rationale": "the last road in"}
        _, (verdict,) = self.refine([drop], run, graph=one_road)
        self.assertEqual(verdict["reason"], "unreachable")

    def test_the_shared_fixture_is_judged_alike_on_both_sides(self):
        fixture = json.loads((FIXTURES / "structural.json").read_text())
        self.assertEqual(fixture["conditions"], refine.conditions())
        for case in fixture["cases"]:
            graph = dict(case["graph"], edges=case["graph"]["edges"] * case["repeat"])
            self.assertEqual(refine.check(graph, fixture["verbs"]), case["fails"], case["name"])
        graph = shipped()
        self.assertEqual(refine.check(graph, {node["id"] for node in graph["nodes"]}), None)
        self.assertEqual(refine.dump(graph), refine.GRAPH.read_text(), "the writer's form is the file's form")


if __name__ == "__main__":
    unittest.main()
