"""F-B1/F-B4: schema is keyword-only and typed; result() waits on a wall clock.

stdlib unittest only: list_subagents/result are faked at the module boundary for the
handle tests, host_request directly for the module-level guard — no ipykernel, no
event loop of the kernel's own. Run with:
PYTHONPATH=python/yi_runtime/src python3 -m unittest discover -q -s python/yi_runtime/tests
"""
from __future__ import annotations

import asyncio
import pathlib
import tempfile
import shutil
import os
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


class BlackboardTests(unittest.IsolatedAsyncioTestCase):
    """D164: put/get/ls over RLM_FAMILY_DIR, and a kernel:// object fetch."""

    def setUp(self) -> None:
        self.family = pathlib.Path(tempfile.mkdtemp(prefix="yi-family-"))
        self.env = mock.patch.dict(
            os.environ,
            {"RLM_FAMILY_DIR": str(self.family), "RLM_SESSION_DIR": "/x/rlm-1/sub-abc"},
        )
        self.env.start()

    def tearDown(self) -> None:
        self.env.stop()
        shutil.rmtree(self.family, ignore_errors=True)

    def test_put_get_and_ls_round_trip_with_a_sidecar(self) -> None:
        sidecar = rlm.put("shard_auth", {"findings": [1, 2, 3]})
        self.assertEqual(sidecar["owner"], "sub-abc")
        self.assertEqual(sidecar["type"], "dict")
        self.assertTrue((self.family / "shard_auth.dill").is_file())
        self.assertEqual(rlm.get("shard_auth"), {"findings": [1, 2, 3]})
        self.assertEqual([entry["name"] for entry in rlm.ls()], ["shard_auth"])
        with self.assertRaises(KeyError):
            rlm.get("nothing")
        with self.assertRaises(ValueError):
            rlm.put("../escape", 1)

    async def test_a_kernel_object_fetch_undills_the_path_the_host_names(self) -> None:
        target = self.family / "main.df.dill"
        with open(target, "wb") as handle:
            rlm._serializer().dump([4, 5], handle)
        calls: list[dict] = []

        async def fake_host_request(kind, payload):
            calls.append(payload)
            return {"path": str(target), "bytes": target.stat().st_size}

        with mock.patch.object(rlm, "host_request", fake_host_request):
            self.assertEqual(await rlm.fetch("kernel://main/df"), [4, 5])
        self.assertEqual(calls, [{"url": "kernel://main/df", "object": True}])

        async def text_host_request(kind, payload):
            return {"text": "[4, 5]"}

        with mock.patch.object(rlm, "host_request", text_host_request):
            self.assertEqual(await rlm.fetch("kernel://main/df", as_text=True), "[4, 5]")


class StatusTests(unittest.IsolatedAsyncioTestCase):
    async def test_status_reads_the_member_list_and_filters_by_name(self) -> None:
        members = [
            {"name": "a", "state": "running", "note": None},
            {"name": "d", "state": "needs_you", "note": "which port?"},
        ]

        async def fake_host_request(kind, payload):
            self.assertEqual(kind, "rlm.status")
            return {"members": members}

        with mock.patch.object(rlm, "host_request", fake_host_request):
            self.assertEqual(await rlm.status(), members)
            self.assertEqual(await rlm.status("d"), [members[1]])

