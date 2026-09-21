"""F-B1/F-B4: schema is keyword-only and typed; result() waits on a wall clock.

stdlib unittest only: list_subagents/result are faked at the module boundary for the
handle tests, host_request directly for the module-level guard — no ipykernel, no
event loop of the kernel's own. Run with:
PYTHONPATH=python/yi_runtime/src python3 -m unittest discover -q -s python/yi_runtime/tests
"""
from __future__ import annotations

import asyncio
import inspect
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


def _wait_reply(state: str, *, name: str = "n", cursor: int = 1) -> dict:
    return {
        "cursor": cursor,
        "changed": [name],
        "updated": [name],
        "states": {name: state},
        "notes": {},
        "timeout_ms": 1000,
        "clamped": False,
    }


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

    async def test_result_uses_wait_and_keeps_its_errors(self) -> None:
        """F0a: the handle blocks in rlm.wait with its own cursor, never a listing poll."""
        signature = inspect.signature(rlm.RLMSpawnHandle.result)
        self.assertEqual(signature.parameters["timeout"].default, 540.0)
        self.assertNotIn("poll", signature.parameters)

        async def never_listed() -> list[rlm.RLMSubagent]:
            raise AssertionError("result must not poll list_subagents")

        seen: list[tuple[float, int | None]] = []

        async def wait_to_finished(timeout: float, cursor: int | None = None) -> dict:
            seen.append((timeout, cursor))
            return _wait_reply("running" if len(seen) == 1 else "finished", cursor=len(seen))

        async def fake_result(*args, **kwargs):
            return {"text": "ok"}

        with mock.patch.object(rlm, "list_subagents", never_listed), mock.patch.object(
            rlm, "wait", wait_to_finished
        ), mock.patch.object(rlm, "result", fake_result):
            self.assertEqual(await _handle().result(), {"text": "ok"})
        self.assertEqual([cursor for _, cursor in seen], [None, 1])
        self.assertLessEqual(seen[0][0], 540.0)

        async def wait_without_the_child(timeout: float, cursor: int | None = None) -> dict:
            return _wait_reply("finished", name="other")

        start = time.monotonic()
        with mock.patch.object(rlm, "list_subagents", never_listed), mock.patch.object(
            rlm, "wait", wait_without_the_child
        ):
            with self.assertRaisesRegex(RuntimeError, "no longer registered"):
                await _handle().result(timeout=5)
        self.assertLess(time.monotonic() - start, 1.0)

        waits: list[float] = []

        async def wait_running(timeout: float, cursor: int | None = None) -> dict:
            waits.append(timeout)
            await asyncio.sleep(min(timeout, 0.02))
            return _wait_reply("running")

        with mock.patch.object(rlm, "list_subagents", never_listed), mock.patch.object(
            rlm, "wait", wait_running
        ):
            with self.assertRaises(TimeoutError):
                await _handle().result(timeout=0.05)
        self.assertTrue(waits and all(asked <= 0.05 for asked in waits))

    async def test_a_child_asking_its_parent_is_collected_not_timed_out(self) -> None:
        """needs_you names a child that ended on ask_user (D165); its answer is collectable."""

        async def wait_needs_you(timeout: float, cursor: int | None = None) -> dict:
            return _wait_reply("needs_you")

        async def fake_result(*args, **kwargs):
            return {"text": "which file?"}

        with mock.patch.object(rlm, "wait", wait_needs_you), mock.patch.object(
            rlm, "result", fake_result
        ):
            self.assertEqual(await _handle().result(timeout=1.0), {"text": "which file?"})

        # needs_you also names a running child blocked on the user: the host refuses
        # its result and the handle keeps waiting.
        seen: list[int | None] = []

        async def wait_blocked_then_finished(timeout: float, cursor: int | None = None) -> dict:
            seen.append(cursor)
            state = "needs_you" if len(seen) == 1 else "finished"
            return _wait_reply(state, cursor=len(seen))

        results: list[int] = []

        async def refuse_then_answer(*args, **kwargs):
            results.append(1)
            if len(results) == 1:
                raise RuntimeError('child "n" is still running')
            return {"text": "ok"}

        with mock.patch.object(rlm, "wait", wait_blocked_then_finished), mock.patch.object(
            rlm, "result", refuse_then_answer
        ):
            self.assertEqual(await _handle().result(timeout=1.0), {"text": "ok"})
        self.assertEqual(seen, [None, 1])
        self.assertEqual(len(results), 2)

    async def test_a_finished_child_forwards_the_schema_as_a_keyword(self) -> None:
        async def fake_wait(timeout: float, cursor: int | None = None) -> dict:
            return _wait_reply("finished")

        calls: list[tuple[tuple, dict]] = []

        async def fake_result(*args, **kwargs):
            calls.append((args, kwargs))
            return {"text": "ok"}

        with mock.patch.object(rlm, "wait", fake_wait), mock.patch.object(
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

    async def test_a_paged_fetch_sends_only_the_keys_it_was_given_and_carries_the_next_offset(self) -> None:
        calls: list[dict] = []

        async def fake_host_request(kind, payload):
            calls.append(payload)
            return {"text": "ab", "next_offset": 2 if "limit" in payload else None}

        with mock.patch.object(rlm, "host_request", fake_host_request):
            whole = await rlm.fetch("local://notes.md")
            page = await rlm.fetch("kernel://sub-1/df", limit=2)
            last = await rlm.fetch("local://notes.md", offset=2)
        self.assertEqual((type(whole), whole), (str, "ab"))
        self.assertEqual((page, page.next_offset, last.next_offset), ("ab", 2, None))
        self.assertEqual(calls[0], {"url": "local://notes.md", "object": False})
        self.assertEqual(calls[1], {"url": "kernel://sub-1/df", "object": False, "limit": 2})
        self.assertEqual(calls[2], {"url": "local://notes.md", "object": False, "offset": 2})


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

    async def test_revoke_sends_the_grace_in_milliseconds(self) -> None:
        sent = []

        async def fake_host_request(kind, payload):
            sent.append((kind, payload))
            return {"revoked": payload["target"]}

        with mock.patch.object(rlm, "host_request", fake_host_request):
            await rlm.revoke("d", grace_s=1.5, reason="out of scope")
        self.assertEqual(sent, [("rlm.revoke", {"target": "d", "grace_ms": 1500, "reason": "out of scope"})])



class PlanOpTests(unittest.IsolatedAsyncioTestCase):
    """F0a: plan.op is one host request; rlm.wait carries a cursor."""

    async def test_plan_op_sends_the_payload_shape(self) -> None:
        calls: list[tuple[str, dict]] = []

        async def fake_host_request(kind, payload):
            calls.append((kind, payload))
            return {"ok": True, "revision": 4, "text": "header"}

        with mock.patch.object(rlm, "host_request", fake_host_request):
            reply = await rlm.plan_op("view", {"full": True})
            self.assertEqual(reply, {"ok": True, "revision": 4, "text": "header"})
            kind, payload = calls[-1]
            self.assertEqual(kind, "plan.op")
            self.assertEqual(set(payload), {"request_id", "op", "args"})
            self.assertEqual(payload["op"], "view")
            self.assertEqual(payload["args"], {"full": True})
            self.assertEqual(len(payload["request_id"]), 36)

            await rlm.plan_op("view")
            self.assertEqual(set(calls[-1][1]), {"request_id", "op"})

            await rlm.plan_op(
                "done", {"id": "t1"}, plan="p", request_id="r-1", expected_revision=0
            )
            self.assertEqual(
                calls[-1][1],
                {
                    "request_id": "r-1",
                    "plan": "p",
                    "expected_revision": 0,
                    "op": "done",
                    "args": {"id": "t1"},
                },
            )

    async def test_wait_sends_the_cursor(self) -> None:
        calls: list[tuple[str, dict]] = []

        async def fake_host_request(kind, payload):
            calls.append((kind, payload))
            return _wait_reply("running")

        with mock.patch.object(rlm, "host_request", fake_host_request):
            self.assertEqual(await rlm.wait(1.5), _wait_reply("running"))
            self.assertEqual(calls[-1], ("rlm.wait", {"timeout_ms": 1500}))
            await rlm.wait(2, cursor=7)
            self.assertEqual(calls[-1][1], {"timeout_ms": 2000, "cursor": 7})
            await rlm.rlm.wait(2, 9)
            self.assertEqual(calls[-1][1], {"timeout_ms": 2000, "cursor": 9})
