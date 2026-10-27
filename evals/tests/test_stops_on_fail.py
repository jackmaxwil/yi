"""The stops-over-FAIL counter and EXP-B3c's verdict, over sessions the real binary wrote under
faux/faux-1 (evals/fixtures/stops_on_fail): no model, no TB task text."""
import contextlib, importlib.util, io, json, shutil, sys, tempfile, unittest
from pathlib import Path

EVALS = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(EVALS))
sys.path.insert(0, str(EVALS / "drivers"))
import stops_on_fail  # noqa: E402
import trials  # noqa: E402

# Two ipython cells, then a final reply: `fail` ends on `FAIL R2` after an all-PASS cell, `pass`
# the reverse, so a counter reading any cell but the last inverts both. `refused` stops over FAIL,
# the todo intercept sends it back, and it stops again over all PASS.
FAIL, PASS, REFUSED = (EVALS / "fixtures" / "stops_on_fail" / f"{name}.jsonl" for name in ("fail", "pass", "refused"))
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

    def test_a_stop_the_host_sent_back_is_not_taken_and_the_one_that_stood_is(self):
        self.assertEqual(stops_on_fail.count(REFUSED), (1, 0))
        lines = ['{"kind": "header", "version": 4}', entry("user"), cell("FAIL R1"), entry("assistant", stopReason="stop"),
                 entry("user", attribution={"host": "job"}), entry("assistant", stopReason="stop"),
                 entry("custom", customType="todo_intercept")]
        with tempfile.TemporaryDirectory() as tmp:
            path = Path(tmp) / "s.jsonl"
            path.write_text("\n".join(lines) + "\n")
            self.assertEqual(stops_on_fail.count(path), (1, 1))

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
            lines = FAIL.read_text().splitlines()
            child = "\n".join([json.dumps({**json.loads(lines[0]), "parentSessionId": "root"}), *lines[1:]]) + "\n"

            def run(name, failing, tasks=B3C.SLICE, sweep="job"):
                job = Path(tmp) / name / sweep
                shutil.rmtree(job, ignore_errors=True)
                for n, task in enumerate(tasks):
                    sessions = job / f"{task}__x{n}" / "agent" / "yi" / "sessions"
                    sessions.mkdir(parents=True)
                    (sessions.parents[2] / "result.json").write_text(json.dumps({"task_name": f"terminal-bench/{task}"}))
                    shutil.copy(FAIL if n < failing else PASS, sessions / "s.jsonl")
                    (sessions / "child.jsonl").write_text(child)
                return {"family": name, "run": name, "jobs": [str(job)]}
            self.assertEqual(B3C.verdict([run("kimi", 2), run("glm-flash", 2)]), "KEEP: kimi 2/12 tasks, glm-flash 2/12 tasks")
            self.assertTrue(B3C.verdict([run("kimi", 2), run("glm-flash", 1)]).startswith("KILL: "))
            refused = {"family": "glm-flash", "run": "refused-before-harbor", "status": "error", "jobs": []}
            self.assertEqual(B3C.verdict([run("kimi", 2), refused]), "UNRESOLVED: kimi 2/12 tasks, glm-flash 0/0 tasks")
            gone = {"family": "glm-flash", "run": "job-dir-cleaned", "jobs": [str(Path(tmp) / "cleaned" / "job")]}
            self.assertEqual(B3C.verdict([run("kimi", 2), gone]), "UNRESOLVED: kimi 2/12 tasks, glm-flash 0/0 tasks")
            self.assertTrue(B3C.verdict([run("kimi", 2)]).startswith("UNRESOLVED: kimi 2/12 tasks, glm-flash 0/0"))
            self.assertEqual(B3C.verdict([run("kimi", 2, B3C.SLICE[:11]), run("glm-flash", 2)]),
                             "UNRESOLVED: kimi 2/11 tasks, glm-flash 2/12 tasks")
            self.assertTrue(B3C.verdict([run("kimi", 2, B3C.SLICE[:11]), run("glm-flash", 1)]).startswith("KILL: "))
            # A re-run under the same run id: each task reads its latest sweep, so two sweeps never sum.
            first, again = run("glm-flash", 2, sweep="old"), run("glm-flash", 0, B3C.SLICE[1:2], sweep="new")
            self.assertEqual(B3C.verdict([run("kimi", 2), first, again]), "KILL: kimi 2/12 tasks, glm-flash 1/12 tasks")

    def test_a_filed_trial_row_names_the_job_the_verdict_reads(self):
        with tempfile.TemporaryDirectory() as tmp:
            job = Path(tmp) / "job"
            trial = job / f"{B3C.SLICE[0]}__x0"
            (trial / "agent" / "yi" / "sessions").mkdir(parents=True)
            (trial / "result.json").write_text(json.dumps({"task_name": f"terminal-bench/{B3C.SLICE[0]}"}))
            shutil.copy(FAIL, trial / "agent" / "yi" / "sessions" / "s.jsonl")
            saved, trials.STORE = trials.STORE, Path(tmp) / "store"
            try:
                with contextlib.redirect_stdout(io.StringIO()):
                    self.assertEqual(trials.main(["rows", str(job), "--run-id", "r"]), 0)
            finally:
                trials.STORE = saved
            filed = [json.loads(line) for line in (Path(tmp) / "store" / "r.jsonl").read_text().splitlines()]
            self.assertEqual(B3C.tasks_on_fail(sorted({row["job"] for row in filed})), {B3C.SLICE[0]: 1})

if __name__ == "__main__":
    unittest.main()
