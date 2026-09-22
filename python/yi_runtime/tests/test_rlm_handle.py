"""A parent blocked on a spawn handle sees what rlm.wait says about the child.

stdlib unittest with a fake rlm.wait, as test_rlm.py does. Run with:
PYTHONPATH=python/yi_runtime/src python3 -m unittest discover -q -s python/yi_runtime/tests
"""
from __future__ import annotations

import asyncio
import pathlib
import unittest
from unittest import mock

import rlm


class HandleTests(unittest.IsolatedAsyncioTestCase):
    async def test_result_surfaces_a_stuck_state_from_wait(self) -> None:
        """A stuck child held the cell to its 540 s deadline with nothing said."""
        handle = rlm.RLMSpawnHandle(
            rlm_child_id="sub-1", name="n", session_dir=pathlib.Path("/tmp"), model="faux/faux-1"
        )
        calls: list[int | None] = []

        async def wait_running_then_stuck(timeout: float, cursor: int | None = None) -> dict:
            calls.append(cursor)
            stuck = len(calls) > 1
            return {
                "cursor": len(calls),
                "changed": ["n"],
                "states": {"n": "stuck" if stuck else "running"},
                "notes": {"n": "same bash call 6 times"} if stuck else {},
            }

        async def never_collected(*args, **kwargs):
            raise AssertionError("a stuck child has no result to collect")

        with mock.patch.object(rlm, "wait", wait_running_then_stuck), mock.patch.object(
            rlm, "result", never_collected
        ):
            with self.assertRaisesRegex(RuntimeError, "stuck .same bash call 6 times."):
                await handle.result(timeout=5.0)
        self.assertEqual(calls, [0, 1], "it kept its own cursor until the state turned")

    async def test_a_fresh_waiter_on_a_finished_child_answers_at_once(self) -> None:
        """Dies with the control: start the wait with no cursor and the host resumes it from the
        model's last bare wait, which already saw the child finish, so it blocks to its timeout."""
        handle = rlm.RLMSpawnHandle(
            rlm_child_id="sub-1", name="n", session_dir=pathlib.Path("/tmp"), model="faux/faux-1"
        )
        seen_by_the_model = 3

        async def host_wait(timeout: float, cursor: int | None = None) -> dict:
            since = seen_by_the_model if cursor is None else cursor
            if since >= seen_by_the_model:
                await asyncio.sleep(timeout)
            return {"cursor": 3, "changed": [], "states": {"n": "finished"}, "notes": {}}

        async def collected(*args, **kwargs) -> dict:
            return {"text": "done"}

        with mock.patch.object(rlm, "wait", host_wait), mock.patch.object(rlm, "result", collected):
            answer = await asyncio.wait_for(handle.result(timeout=5.0), timeout=1.0)
        self.assertEqual(answer, {"text": "done"})


if __name__ == "__main__":
    unittest.main()
