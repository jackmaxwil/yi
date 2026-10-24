"""The stops-over-FAIL counter and EXP-B3c's verdict, over sessions the real binary wrote under
faux/faux-1 (evals/fixtures/stops_on_fail): no model, no TB task text."""
import importlib.util, json, os, shutil, sys, tempfile, unittest
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))
import stops_on_fail  # noqa: E402

# Two ipython cells, then a final reply: `fail` ends on `FAIL R2` after an all-PASS cell, `pass`
# the reverse, so a counter reading any cell but the last inverts both.
FAIL, PASS = (EVALS / "fixtures" / "stops_on_fail" / f"{name}.jsonl" for name in ("fail", "pass"))
SPEC = importlib.util.spec_from_file_location("kill_x_b3c", EVALS / "kill" / "x-b3c.py")
B3C = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(B3C)


def entry(role, **message):
    return json.dumps({"kind": "entry", "type": "message", "message": {"role": role, **message}})


def cell(stdout):
    return entry("toolResult", toolName="ipython", isError=False, content=[{"type": "text", "text": stdout}],
                 details={"stdout": stdout})


class StopsOnFail(unittest.TestCase):
    def test_a_stop_after_a_last_cell_that_printed_fail_counts_and_one_after_all_pass_does_not(self):
        self.assertEqual(stops_on_fail.count(FAIL), (1, 1))
        self.assertEqual(stops_on_fail.count(PASS), (1, 0))

    def test_an_accepted_answer_is_one_stop_and_a_held_one_is_none(self):
        lines = ['{"kind": "header", "version": 4}', entry("user"), cell("PASS R1\nFAILED to import is no verdict"),
                 entry("assistant", stopReason="stop"), entry("user"), cell("PASS R1\nFAIL R2"),
                 entry("toolResult", toolName="answer", isError=True), cell("PASS R1\nPASS R2"),
                 entry("toolResult", toolName="answer"), entry("assistant", stopReason="stop")]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "s.jsonl"
            path.write_text("\n".join(lines) + "\n")
            self.assertEqual(stops_on_fail.count(path), (2, 0))

    def test_the_verdict_keeps_at_two_tasks_per_family_kills_at_one_and_waits_for_a_short_family(self):
        with tempfile.TemporaryDirectory() as tmp:
            saved = os.environ.get("TBV4_RUNS_ROOT")
            os.environ["TBV4_RUNS_ROOT"] = tmp
            try:
                lines = FAIL.read_text().splitlines()
                child = "\n".join([json.dumps({**json.loads(lines[0]), "parentSessionId": "root"}), *lines[1:]]) + "\n"

                def run(name, failing, tasks=B3C.SLICE):
                    shutil.rmtree(Path(tmp) / name, ignore_errors=True)
                    for n, task in enumerate(tasks):
                        sessions = Path(tmp) / name / "job" / f"{task}__x{n}" / "agent" / "yi" / "sessions"
                        sessions.mkdir(parents=True)
                        (sessions.parents[2] / "result.json").write_text(json.dumps({"task_name": f"terminal-bench/{task}"}))
                        shutil.copy(FAIL if n < failing else PASS, sessions / "s.jsonl")
                        (sessions / "child.jsonl").write_text(child)
                    return {"family": name, "run": name}
                self.assertEqual(B3C.verdict([run("kimi", 2), run("glm-flash", 2)]), "KEEP: kimi 2/12 tasks, glm-flash 2/12 tasks")
                self.assertTrue(B3C.verdict([run("kimi", 2), run("glm-flash", 1)]).startswith("KILL: "))
                self.assertTrue(B3C.verdict([run("kimi", 2)]).startswith("UNRESOLVED: kimi 2/12 tasks, glm-flash 0/0"))
                self.assertTrue(B3C.verdict([run("kimi", 2, B3C.SLICE[:11]), run("glm-flash", 1)])
                                .startswith("UNRESOLVED: kimi 2/11"))
            finally:
                os.environ.pop("TBV4_RUNS_ROOT") if saved is None else os.environ.update(TBV4_RUNS_ROOT=saved)


if __name__ == "__main__":
    unittest.main()
