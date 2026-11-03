"""The kernel-side contract of ``fetch(url)`` and ``bash(cmd)`` against a stubbed host.

stdlib unittest with ``rlm.host_request`` replaced: no kernel, no host. Run with:
PYTHONPATH=python/yi_runtime/src python3 -m unittest discover -q -s python/yi_runtime/tests
"""
from __future__ import annotations

import os
import unittest
from unittest import mock

import rlm


class KernelLocalNameTests(unittest.TestCase):
    def local_name(self, url: str, session_dir: str | None) -> str | None:
        env = {} if session_dir is None else {"RLM_SESSION_DIR": session_dir}
        with mock.patch.dict(os.environ, env):
            if session_dir is None:
                os.environ.pop("RLM_SESSION_DIR", None)
            return rlm._kernel_local_name(url)

    def test_the_owner_kernel_resolves_only_its_own_flat_names(self) -> None:
        root = "/tmp/session-root"
        self.assertEqual(self.local_name("kernel://x", root), "x")
        self.assertEqual(self.local_name("kernel://main/x", root), "x")
        self.assertIsNone(self.local_name("kernel://other/x", root))
        self.assertIsNone(self.local_name("kernel://main/a/b", root))
        self.assertIsNone(self.local_name("kernel://main/", root))
        self.assertIsNone(self.local_name("local://x", root))

    def test_a_child_kernel_forwards_every_kernel_url_to_the_host(self) -> None:
        child = "/tmp/session-root/sub-abc"
        self.assertIsNone(self.local_name("kernel://x", child))
        self.assertIsNone(self.local_name("kernel://main/x", child))

    def test_a_kernel_with_no_session_dir_is_the_owner(self) -> None:
        self.assertEqual(self.local_name("kernel://x", None), "x")


class BashFetchTests(unittest.IsolatedAsyncioTestCase):
    def test_bash_refuses_an_empty_command(self) -> None:
        with self.assertRaises(ValueError):
            rlm.bash("   ")

    async def test_a_killed_handle_releases_its_job_and_makes_no_further_requests(self) -> None:
        calls: list[str] = []
        state = {"running": True, "released": False}

        async def stub(request_type, payload=None):
            calls.append(request_type)
            if request_type == "exec.spawn":
                return {"job_id": 7}
            if request_type == "exec.tail":
                self.assertEqual(payload["cursor"], 0)
                return {"text": "hi", "next": 2, "dropped": 3}
            if request_type == "exec.poll":
                return {"running": state["running"], "exit_code": None, "killed": False}
            if request_type == "exec.kill":
                state["running"] = False
                return {"outcome": "signalled"}
            if request_type == "exec.release":
                state["released"] = True
                return {
                    "command": "sleep 5",
                    "output": "hi",
                    "exit_code": None,
                    "killed": True,
                    "running": False,
                }
            raise AssertionError(request_type)

        with mock.patch.object(rlm, "host_request", stub):
            handle = rlm.bash("sleep 5")
            tail = await handle.tail()
            self.assertTrue(tail.startswith("[... 3 bytes trimmed") and tail.endswith("hi"), tail)
            self.assertIs((await handle.poll())["running"], True)
            report = await handle.kill()
            self.assertIs(report["killed"], True)
            self.assertTrue(state["released"], "kill must release the host-side job")
            sent = len(calls)
            self.assertEqual(await handle, report, "a second await returns the cached report")
            self.assertEqual(len(calls), sent, "a released handle makes no further host requests")
            self.assertEqual(await handle.tail(), "")

    async def test_fetch_refuses_an_empty_url(self) -> None:
        with self.assertRaises(TypeError):
            await rlm.fetch("")


if __name__ == "__main__":
    unittest.main()
