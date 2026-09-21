"""yi.mail: messages between family members, over ``rlm``.

    send      one message; the receipt says what the host did with it
    request   send and wait for the reply
    inbox     every envelope the host accepted for you, read from your own history

The host writes an envelope to the receiver's inbox before it delivers anything,
so a receipt of ``inboxed`` is a message kept, not a message lost: the receiver
finds it with ``inbox()`` whenever it next runs. Reading an inbox never changes
it, and ``since`` is the ``seq`` of the last entry you handled.

    from yi import mail
    answer = await mail.request("tests", "which suite is red?", timeout=120)
    for entry in await mail.inbox(since=0):
        if entry["data"]["kind"] == "request":
            await mail.send(entry["data"]["from"], "on it", reply_to=entry["data"]["id"])
"""
from __future__ import annotations

import json
from typing import Any

import rlm

send = rlm.send
request = rlm.request


async def inbox(agent: str = "self", since: int = 0, limit: int | None = None) -> list[dict[str, Any]]:
    """The envelopes in ``agent``'s inbox after entry ``since``, oldest first.

    Each item is a history entry: ``seq`` is the cursor to pass back as ``since``,
    and ``data`` is the envelope ``{id, from, to, kind, conversation, inReplyTo,
    seq, sentAt, body, ref}`` (``data["seq"]`` orders one sender's messages to
    you; it is not the cursor). ``agent`` defaults to you; a parent may name a
    child, finished or reaped, to see what it was sent. ``limit`` keeps the first few.

        waiting = await inbox(since=0)
    """
    url = f"history://{agent}/since/{int(since)}/custom/agent_message"
    page = await rlm.fetch(url, offset=None if limit is None else 0, limit=limit)
    return [json.loads(line) for line in page.splitlines() if line]
