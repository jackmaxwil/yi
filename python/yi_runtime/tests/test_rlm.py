"""F-B1/F-B4: schema is keyword-only and typed; result() waits on a wall clock.

stdlib unittest only: list_subagents/result are faked at the module boundary for the
handle tests, host_request directly for the module-level guard — no ipykernel, no
event loop of the kernel's own. Run with:
PYTHONPATH=python/yi_runtime/src python3 -m unittest discover -q -s python/yi_runtime/tests
"""
from __future__ import annotations

import asyncio
import pathlib
import time
import unittest
from unittest import mock

import rlm


def _handle() -> rlm.RLMSpawnHandle:
    return rlm.RLMSpawnHandle(
        rlm_child_id="sub-1", name="n", session_dir=pathlib.Path("/tmp"), model="faux/faux-1"
    )


def _entry(status: str) -> rlm.RLMSubagent:
    return rlm.RLMSubagent(
        rlm_child_id="sub-1",
        active_session_id=None,
        session_id=None,
        session_name="n",
        session_dir=pathlib.Path("/tmp"),
        status=status,
    )


class ResultSignatureTests(unittest.IsolatedAsyncioTestCase):
    def test_result_refuses_a_positional_schema(self) -> None:
        h = _handle()
        with self.assertRaises(TypeError):
            h.result(30)
        with self.assertRaises(TypeError):
            rlm.result("n", 30)
        with self.assertRaises(TypeError):
            rlm.rlm.result("n", 30)

    async def test_result_refuses_a_non_dict_schema_before_any_host_round_trip(self) -> None:
        calls: list[int] = []

        async def fake() -> list[rlm.RLMSubagent]:
            calls.append(1)
            return []

        with mock.patch.object(rlm, "list_subagents", fake):
            with self.assertRaises(TypeError):
                await _handle().result(schema=30)
        self.assertEqual(calls, [])

    async def test_result_deadline_is_wall_clock_not_poll_count(self) -> None:
        calls: list[int] = []

        async def fake() -> list[rlm.RLMSubagent]:
            calls.append(1)
            await asyncio.sleep(0.03)
            return [_entry("running")]

        with mock.patch.object(rlm, "list_subagents", fake):
            with self.assertRaises(TimeoutError):
                await _handle().result(timeout=0.05, poll=0.01)
        self.assertLessEqual(len(calls), 3)

    async def test_result_fails_fast_when_the_child_is_no_longer_registered(self) -> None:
        calls: list[int] = []

        async def fake() -> list[rlm.RLMSubagent]:
            calls.append(1)
            return []

        start = time.monotonic()
        with mock.patch.object(rlm, "list_subagents", fake):
            with self.assertRaisesRegex(RuntimeError, "no longer registered"):
                await _handle().result(timeout=5, poll=0.01)
        elapsed = time.monotonic() - start
        self.assertEqual(len(calls), 1)
        self.assertLess(elapsed, 1.0)

    async def test_a_finished_child_forwards_the_schema_as_a_keyword(self) -> None:
        async def fake_list() -> list[rlm.RLMSubagent]:
            return [_entry("completed")]

        calls: list[tuple[tuple, dict]] = []

        async def fake_result(*args, **kwargs):
            calls.append((args, kwargs))
            return {"text": "ok"}

        with mock.patch.object(rlm, "list_subagents", fake_list), mock.patch.object(
            rlm, "result", fake_result
        ):
            await _handle().result(schema={"type": "object"})
        self.assertEqual(len(calls), 1)
        args, kwargs = calls[0]
        self.assertEqual(args, ("sub-1",))
        self.assertEqual(kwargs, {"schema": {"type": "object"}})

    async def test_module_result_refuses_a_non_dict_schema_before_any_host_round_trip(
        self,
    ) -> None:
        calls: list[tuple[tuple, dict]] = []

        async def fake_host_request(*args, **kwargs):
            calls.append((args, kwargs))
            return {}

        with mock.patch.object(rlm, "host_request", fake_host_request):
            with self.assertRaises(TypeError):
                await rlm.result("n", schema=30)
        self.assertEqual(calls, [])


if __name__ == "__main__":
    unittest.main()
