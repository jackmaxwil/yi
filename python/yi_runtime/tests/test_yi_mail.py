"""F2a: `rlm.send` keeps its old call and gains the envelope's words; `yi.mail` reads an inbox."""
from __future__ import annotations

import asyncio
import json
import unittest

import rlm
from yi import mail


class Mail(unittest.TestCase):
    def setUp(self) -> None:
        self.sent: list[tuple[str, dict]] = []
        self.replies: dict[str, dict] = {}
        self.real = rlm.host_request

        async def request(kind: str, payload: dict | None = None) -> dict:
            self.sent.append((kind, payload or {}))
            return self.replies.get(kind, {})

        rlm.host_request = request

    def tearDown(self) -> None:
        rlm.host_request = self.real

    def test_a_plain_send_is_the_payload_it_always_was(self) -> None:
        asyncio.run(rlm.send("tests", "status?"))
        asyncio.run(rlm.followup("tests", "now"))
        self.assertEqual(self.sent[0], ("agent_message.send", {"target": "tests", "message": "status?", "followup": False}))
        self.assertEqual(self.sent[1][1]["followup"], True)

    def test_a_reply_names_the_request_and_a_request_carries_its_timeout(self) -> None:
        asyncio.run(mail.send("parent", "the fetch suite", reply_to="parent-1"))
        self.assertEqual(self.sent[0][1]["reply_to"], "parent-1")
        self.assertNotIn("kind", self.sent[0][1], "the host reads a reply off reply_to")
        self.replies["agent_message.request"] = {"reply": "green"}
        answer = asyncio.run(mail.request("tests", "which suite is red?", timeout=2))
        self.assertEqual(answer["reply"], "green")
        self.assertEqual(self.sent[1], ("agent_message.request", {"target": "tests", "message": "which suite is red?", "timeout_ms": 2000}))
        with self.assertRaises(ValueError):
            asyncio.run(mail.request("tests", ""))

    def test_an_inbox_is_a_filtered_read_of_history_and_parses_each_envelope(self) -> None:
        entry = {"type": "custom", "customType": "agent_message", "seq": 9, "data": {"id": "parent-1", "kind": "request", "body": "hi"}}
        self.replies["fetch"] = {"text": json.dumps(entry) + "\n" + json.dumps(entry)}
        waiting = asyncio.run(mail.inbox(since=4))
        self.assertEqual([item["data"]["id"] for item in waiting], ["parent-1", "parent-1"])
        kind, payload = self.sent[0]
        self.assertEqual((kind, payload["url"]), ("fetch", "history://self/since/4/custom/agent_message"))
        self.assertNotIn("limit", payload, "an unpaged inbox read asks for no page")
        asyncio.run(mail.inbox("tests", limit=1))
        self.assertEqual((self.sent[1][1]["url"], self.sent[1][1]["limit"]), ("history://tests/since/0/custom/agent_message", 1))

    def test_receive_waits_on_the_host_and_hands_back_the_envelopes(self) -> None:
        """Dies with receive gone: beta, told to wait for alpha, polled the filesystem instead."""
        self.replies["rlm.receive"] = {"envelopes": [{"id": "alpha-1", "from": "alpha", "body": "391"}]}
        got = asyncio.run(mail.receive(timeout=5))
        self.assertEqual([(env["from"], env["body"]) for env in got], [("alpha", "391")])
        self.assertEqual(self.sent[0], ("rlm.receive", {"timeout_ms": 5000}))
        self.assertIn("receive", rlm.__all__)


if __name__ == "__main__":
    unittest.main()
