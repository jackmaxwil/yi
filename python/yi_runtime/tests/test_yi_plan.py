"""F1a: create, the source record, the scheduler and its lease (plan sections 8.2 and 8.3)."""
from __future__ import annotations

import asyncio
import unittest

import yi.plan
from fake_host import FakeHost, refusal
from yi import Plan, PlanError, RunActive, Writer, cmd, contract, in_order, schema

REPORT = {"type": "object", "required": ["passed"]}


class Plans(unittest.IsolatedAsyncioTestCase):
    def tearDown(self) -> None:
        yi.plan._CELL = None

    async def test_a_retried_create_request_is_the_same_plan(self) -> None:
        host = FakeHost()
        first = await Plan.create("ship logrotate-lite", request_id="create-01")
        again = await Plan.create("ship logrotate-lite", request_id="create-01")
        self.assertEqual((again.id, len(host.plans), host.ops()), (first.id, 1, ["init"]))
        with self.assertRaises(PlanError) as reused:
            await Plan.create("ship something else", request_id="create-01")
        self.assertEqual(reused.exception.kind, "request_id_reused")
        with self.assertRaises(PlanError) as guessed:
            await Plan.create("ship logrotate-lite")
        self.assertEqual(guessed.exception.kind, "plan_exists", "create never adopts a plan by its text")

    async def test_a_cell_is_recorded_once_before_its_first_effect(self) -> None:
        host = FakeHost()

        class Cell:
            raw_cell = "plan = await Plan.create('ship it')\nawait plan.todo(key='a')\n"

        yi.plan._on_cell(Cell)
        plan = await Plan.create("ship it")
        self.assertEqual(host.ops(), ["init", "program"], "a cell that only creates is recorded too")
        await plan.todo(key="a")
        await plan.todo(key="a")
        self.assertEqual(host.ops(), ["init", "program", "append"])
        digest = host.journal[1][1]["source_ref"]["digest"]
        self.assertEqual(host.blobs[digest], Cell.raw_cell)

        yi.plan._on_cell(Cell)
        await (await Plan.attach(plan.id)).refresh()
        self.assertEqual(host.ops().count("program"), 1, "a cell that only reads records nothing")
        await plan["a"].start()
        self.assertEqual(host.ops()[-2:], ["program", "start"], "the record precedes the effect")

    async def test_a_child_records_no_cell_and_still_submits_its_product(self) -> None:
        """The submitting agent is a child by construction, so the library road must be its own."""

        class ChildHost(FakeHost):
            def plan_op(self, payload: dict) -> dict:
                if payload["op"] == "program":
                    return refusal("not_owner", "only the plan owner stores artifacts", "not_owner")
                return super().plan_op(payload)

        host = ChildHost()
        plan = await Plan.create("ship the seam end to end")
        await plan.todo(key="cut", label="cut the release")
        await plan["cut"].start()

        class Cell:
            raw_cell = "await plan['cut'].submit({'passed': 12})\n"

        yi.plan._on_cell(Cell)
        url = await plan["cut"].submit({"passed": 12})
        self.assertEqual(host.ops()[-1], "submit", host.ops())
        self.assertNotIn("program", host.ops(), "a child records no cell in its parent's program")
        self.assertTrue(url.startswith(f"plan://{plan.id}/artifacts/"), url)
        self.assertIn('{"passed":12}', host.blobs.values())
        self.assertEqual(host.journal[-1][1]["attempt"], 1)

    async def test_the_example_runs_end_to_end(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship logrotate-lite with a packaged tarball", request_id="create-01")

        async def freeze_surface() -> dict:
            return {"passed": 3}

        freeze = await plan.todo(
            key="freeze",
            label="freeze the CLI surface",
            run=freeze_surface,
            accept=contract(schema(REPORT, critical=True)),
        )
        tests = await plan.todo(
            key="tests",
            label="write the test suite",
            after=[freeze],
            delegate=Writer(
                accept=contract(cmd("pytest -q tests/", critical=True), schema(REPORT, critical=True)),
                deny_write=["docs/"],
            ),
        )
        child = f"{plan.id}/tests"
        host.children[child] = "finished"
        host.results[child] = {"text": "{}", "json": {"passed": 12}}
        run = await plan.run(budget="2h")
        self.assertEqual(run.outcome, "verified_success", await run.status())
        self.assertEqual(host.spawns, 1)
        for todo in (freeze, tests):
            self.assertTrue(todo._doc["output"].startswith(f"plan://{plan.id}/artifacts/"), todo._doc)
        self.assertIn('{"passed":12}', host.blobs.values())

    async def test_a_writer_without_accept_is_refused_before_the_host(self) -> None:
        """A worktree writer with no contract never reaches the host: the library refuses it first."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        with self.assertRaises(TypeError):
            Writer()
        with self.assertRaises(TypeError) as refused:
            await plan.todo(key="tests", delegate=Writer(accept=None))
        self.assertEqual(str(refused.exception), "a worktree Writer needs accept=")
        self.assertEqual(host.ops(), ["init"], "nothing was sent")
        inline = await plan.todo(key="inline", delegate=Writer(accept=None, isolation=None))
        self.assertNotIn("contract", inline._doc, "an inline writer may still run on the owner's word")
        todo = await plan.todo(key="apart", delegate=Writer(accept=contract(cmd("true", critical=True))))
        self.assertEqual(todo._doc["contract"]["class"], "writer", "the role's accept is the todo's contract")

    async def test_a_writer_takes_a_plain_command_and_stores_a_long_note(self) -> None:
        """Dies with `Writer(accept=str)` refused, or an over-cap note sent whole for the host to refuse."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        note = "Parse the ledger. " * 100
        writer = Writer(accept="grep -qx alpha alpha.txt", note=note)
        todo = await plan.todo(key="quota", delegate=writer)
        self.assertTrue(todo._doc["contract"]["items"][0]["critical"])
        self.assertTrue(any('"command":"grep -qx alpha alpha.txt"' in text for text in host.blobs.values()))
        delegation = todo._doc["delegation"]
        self.assertLessEqual(len(delegation["note"].encode()), 1024)
        self.assertIn(note, host.blobs.values(), "the whole note is in the plan's store")
        self.assertTrue(delegation["note_ref"]["digest"].startswith("sha256:"))
        again = await plan.todo(key="quota", delegate=writer)
        self.assertEqual(again.label, todo.label, "the same declaration is no drift")

    async def test_a_red_verdict_fails_the_attempt_through_the_engine_and_a_stuck_child_is_waited_on(self) -> None:
        """Dies with the control: send `done` from the settle and the library re-runs the verifier
        the engine already ran, once per pass, for as long as the todo stays running."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="tests", delegate=Writer(accept=contract(cmd("false", critical=True))))
        host.verdicts[todo.label] = "fail"
        host.children[f"{plan.id}/tests"] = "stuck"
        run = await plan.run(budget=30, detach=True)
        await asyncio.sleep(0.1)
        self.assertEqual((await run.status())["states"], {"tests": "running"}, "stuck is not failed")
        host.children[f"{plan.id}/tests"] = "finished"
        await run
        self.assertEqual((run.outcome, todo._doc["state"]), ("failed", "failed"))
        self.assertIn("contract refused", todo._doc["cause"])
        self.assertEqual([op for op, _ in host.engine_ops], ["submit", "fail"])
        self.assertFalse({"done", "submit", "fail"} & set(host.sent), "the library sent no step of its own")

    async def test_a_queued_child_is_waited_on(self) -> None:
        """Dies with the control: read `queued` as unknown and an admitted child blocks its todo on you."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="tests", delegate=Writer(accept=contract(cmd("true", critical=True))))
        host.children[f"{plan.id}/tests"] = "queued"
        run = await plan.run(budget=30, detach=True)
        await asyncio.sleep(0.1)
        self.assertEqual((await run.status())["states"], {"tests": "running"}, "queued is not a decision")
        host.children[f"{plan.id}/tests"] = "finished"
        await run
        self.assertEqual((run.outcome, todo._doc["state"]), ("verified_success", "done"))

    async def test_a_reaped_child_is_read_not_blocked(self) -> None:
        """Dies with the control: block a running todo whose child the host no longer lists, and
        the engine's accept, landing between the read and the block, refuses it (10 in one run)."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="t", delegate=Writer(accept=contract(cmd("true", critical=True))))
        child = f"{plan.id}/t"
        host.results[child] = {"text": "{}", "json": {}}
        loop = asyncio.get_running_loop()
        loop.call_later(0.05, host.children.pop, child)
        loop.call_later(0.2, host.children.__setitem__, child, "finished")
        run = await asyncio.wait_for(plan.run(budget=30), timeout=5)
        self.assertEqual((run.outcome, todo._doc["state"]), ("verified_success", "done"))
        self.assertNotIn("block", host.sent, "the library writes nothing on a todo the engine owns")

    async def test_a_verdict_that_judges_no_product_leaves_the_attempt_alone(self) -> None:
        """Dies with the control: fail on every refusal and an abstained todo is failed; with no
        budget, wait on a finished child the engine left running and the run never returns."""
        for outcome, child, state in (("abstain", "finished", "running"), (None, "needs_you", "blocked")):
            with self.subTest(outcome or child):
                host = FakeHost()
                plan = await Plan.create("ship it")
                todo = await plan.todo(key="t", delegate=Writer(accept=contract(cmd("true", critical=True))))
                if outcome is not None:
                    host.verdicts[todo.label] = outcome
                host.children[f"{plan.id}/t"] = child
                run = await asyncio.wait_for(plan.run(), timeout=5)
                self.assertEqual((run.outcome, todo._doc["state"]), ("unresolved", state))
                self.assertNotIn("done", host.sent, "a verdict the engine left is never re-sent")
                yi.plan._RUNS.clear()

    async def test_a_live_asking_child_keeps_its_todo_running(self) -> None:
        """Dies with settle blocking the todo on `needs_you`: the child kept working, its product
        was never submitted, and unblocking the todo would have dispatched a fresh child."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="t", delegate=Writer(accept=contract(cmd("true", critical=True))))
        child = f"{plan.id}/t"
        host.children[child] = "needs_you"
        host.notes[child] = "asks t-1: Which file name?"
        host.results[child] = {"text": "{}", "json": {}}
        asyncio.get_running_loop().call_later(0.1, host.children.__setitem__, child, "finished")
        run = await asyncio.wait_for(plan.run(budget=30), timeout=5)
        self.assertEqual((run.outcome, todo._doc["state"]), ("verified_success", "done"))
        self.assertNotIn("block", host.sent, "an asking child's todo is never blocked")

    async def test_a_todo_whose_child_asks_raises_its_question(self) -> None:
        """Dies with the loop: the next wait slept up to 300 s while this cell was the only
        answerer, and the child's ask timed out onto its default."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="t", delegate=Writer(accept=contract(cmd("true", critical=True))))
        child = f"{plan.id}/t"
        host.children[child] = "needs_you"
        host.notes[child] = "asks t-1: Which file name?"
        host.results[child] = RuntimeError(f"{child} is still running")
        with self.assertRaisesRegex(PlanError, "asks t-1: Which file name.*reply_to"):
            await asyncio.wait_for(todo.result(timeout=5), timeout=5)

    async def test_a_child_asking_you_something_is_collected_not_raised(self) -> None:
        """Dies with the control: hand `rlm.result` a `needs_you` child once and it raises."""
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="t", delegate=Writer(accept=contract(cmd("true", critical=True))))
        self.assertEqual(todo._doc["state"], "running", "the engine starts it at its declaration")
        child = f"{plan.id}/t"
        host.children[child] = "needs_you"
        host.results[child] = RuntimeError(f"{child} is still running")
        asyncio.get_running_loop().call_later(0.05, host.results.__setitem__, child, {"text": "done"})
        self.assertEqual(await todo.result(timeout=5), {"text": "done"})

    async def test_an_engine_refusal_is_not_read_as_a_wait_for_a_slot(self) -> None:
        """Dies with the control: read every idle delegated todo as `admission` and the shape stops
        at it, the inline todo behind it never runs, and the engine's reason is lost."""
        host = FakeHost()
        host.unstartable["deploy"] = "criterion sha256:0 is not in the store"
        plan = await Plan.create("ship it")
        ran = []

        async def notes() -> None:
            ran.append("notes")

        await plan.todo(key="deploy", delegate=Writer(accept=contract(cmd("true", critical=True))))
        await plan.todo(key="notes", run=notes)
        run = await plan.run(budget=5)
        self.assertEqual(ran, ["notes"], "the todo behind the refused one runs")
        refusal = run.refusals["deploy"]
        self.assertEqual(refusal.kind, "not_started")
        self.assertIn("criterion sha256:0 is not in the store", str(refusal))

    async def test_what_the_engine_left_is_read_by_label_not_by_its_prose(self) -> None:
        """Dies with the control: match the notice text and a label with a quote or a backslash,
        which the engine writes escaped, is never found: the run waits out its budget on the
        todo it was left and loses the reason a start was refused."""
        label = 'ship "it" \\ now'
        host = FakeHost()
        host.unstartable[f"deploy: {label}"] = "criterion sha256:0 is not in the store"
        plan = await Plan.create("ship it")
        await plan.todo(key="deploy", label=label, delegate=Writer(accept=contract(cmd("true", critical=True))))

        async def notes() -> None:
            return None

        await plan.todo(key="notes", run=notes)
        run = await plan.run(budget=5)
        self.assertIn("criterion sha256:0 is not in the store", str(run.refusals), run.refusals)
        yi.plan._RUNS.clear()
        host = FakeHost()
        plan = await Plan.create("ship it")
        todo = await plan.todo(key="t", label=label, delegate=Writer(accept=contract(cmd("true", critical=True))))
        host.verdicts[todo.label] = "abstain"
        host.children[f"{plan.id}/t"] = "finished"
        run = await asyncio.wait_for(plan.run(), timeout=5)
        self.assertEqual((run.outcome, todo._doc["state"]), ("unresolved", "running"))

    async def test_duplicate_run_calls_attach_or_refuse(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship it")
        await plan.todo(key="tests", delegate=Writer(accept=contract(cmd("true", critical=True))))
        first = await plan.run(budget=30, detach=True)
        second = await plan.run(budget=30, detach=True)
        self.assertIs(second, first, "the same shape attaches to the running scheduler")

        async def other(plan, run) -> None:
            await in_order(plan, run)

        with self.assertRaises(RunActive) as refused:
            await plan.run(shape=other, detach=True)
        self.assertEqual(refused.exception.run_id, first.id)

        host.children[f"{plan.id}/tests"] = "finished"
        await first
        self.assertEqual((first.outcome, host.spawns), ("verified_success", 1))
        third = await plan.run(shape=other, budget=5)
        self.assertIsNot(third, first, "a finished run holds no lease")

    async def test_stop_cancels_what_is_active_and_reports_what_remains(self) -> None:
        host = FakeHost()
        plan = await Plan.create("ship it")
        started = asyncio.Event()

        async def forever() -> None:
            started.set()
            await asyncio.sleep(3600)

        await plan.todo(key="inline", run=forever)
        await plan.todo(key="child", delegate=Writer(accept=contract(cmd("true", critical=True))))
        await plan.todo(key="later", after=["child"], delegate=Writer(accept=contract(cmd("true", critical=True))))
        run = await plan.run(detach=True)
        await started.wait()
        status = await run.stop(scope="cancel_active")
        self.assertEqual(status["outcome"], "cancelled")
        self.assertEqual(status["states"], {"inline": "failed", "child": "failed", "later": "pending"})
        self.assertIn(("fail", {"label": "child", "cause": "cancelled"}), host.journal, "the fail reaps the child")
        self.assertEqual(host.requests["rlm.interrupt"], 0, "no interrupt hands its end to the engine first")

    async def test_decompose_resolves_an_edge_among_its_own_batch(self) -> None:
        """Dies with the control: look a sibling up in the parent plan and the docstring example raises."""
        FakeHost()
        plan = await Plan.create("ship it")
        rotation = await plan.todo(key="rotation")
        await rotation.start()
        sub = await rotation.decompose([{"key": "parse"}, {"key": "emit", "after": ["parse"]}])
        self.assertEqual([todo.key for todo in sub.todos], ["parse", "emit"])
        self.assertEqual(sub["emit"]._doc["after"], ["parse"], "a sibling edge names the batch, not the parent plan")


if __name__ == "__main__":
    unittest.main()
