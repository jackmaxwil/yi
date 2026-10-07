"""Drive Yi's subagents from any Python program, with no model turn at the root.

    from yi_client import Yi
    with Yi("/path/to/repo", model="openrouter/z-ai/glm-5.3-flash:exacto") as yi:
        answers = yi.ask_all([f"What does {f} do?" for f in files], partition=...)
        yi.run("handle = await rlm.run('fix the bug', role='worker')")

`Yi` starts `yi acp` on stdio, opens one session, and runs code in that session's kernel through
`_yi/kernel_execute`, so every child is spawned and recorded exactly as from a model-run cell.
Standard library only; one file, so a program can vendor it.
"""
from __future__ import annotations

import json
import os
import subprocess
import tempfile
import threading
from typing import Any, Iterable


class YiError(RuntimeError):
    """A refused request or a failed cell; the message is Yi's own text."""


class Yi:
    def __init__(self, cwd: str, *, model: str | None = None, mode: str = "auto", yi: str = "yi",
                 args: Iterable[str] = (), env: dict[str, str] | None = None):
        command = [yi, "acp", "--cwd", str(cwd), f"--{mode}", *args]
        if model:
            command[2:2] = ["--model", model]
        self._proc = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                      text=True, env={**os.environ, **(env or {})})
        self._lock = threading.Lock()
        self._serial = 0
        self._request("initialize", {"protocolVersion": 2})
        self.session = self._request("session/new", {"cwd": str(cwd), "mcpServers": []})["sessionId"]

    def __enter__(self) -> "Yi":
        return self

    def __exit__(self, *_: Any) -> None:
        self.close()

    def close(self) -> None:
        if self._proc.poll() is None:
            try:
                self.run("while True:\n"
                         "    reply = await rlm.wait(300)\n"
                         "    if reply['state'] in ('settled', 'asks'):\n"
                         "        break\n")
            except YiError:
                pass
            self._proc.stdin.close()
            try:
                self._proc.wait(timeout=10)
            except subprocess.TimeoutExpired:
                self._proc.kill()

    def run(self, code: str) -> str:
        """Run one cell (top-level await allowed) and return its output; a failed cell raises."""
        with self._lock:
            call = self._request("_yi/kernel_execute", {"sessionId": self.session, "code": code})["callId"]
            while True:
                update = self._frame().get("params", {}).get("update", {})
                status = update.get("status")
                if update.get("toolCallId") == call and status not in (None, "pending", "in_progress"):
                    text = "".join(part.get("content", {}).get("text", "") for part in update.get("content", []))
                    if status != "completed":
                        raise YiError(text)
                    return text

    def eval(self, expression: str) -> Any:
        """The JSON value of one expression, carried through a file so no output cap cuts it."""
        with tempfile.TemporaryDirectory() as scratch:
            path = os.path.join(scratch, "value.json")
            self.run(f"import json as _yi_json\n_yi_value = {expression}\n"
                     f"with open({path!r}, 'w') as _yi_file:\n"
                     f"    _yi_json.dump(_yi_value, _yi_file, default=str)\n")
            with open(path) as file:
                return json.load(file)

    def ask(self, question: str, **kwargs: Any) -> Any:
        """One reader child's answer: `rlm.ask` with these keyword arguments."""
        return self.eval(f"await rlm.ask(**_yi_json.loads({json.dumps({'question': question, **kwargs})!r}))")

    def ask_all(self, questions: Iterable[str], **kwargs: Any) -> list[Any]:
        """Many readers at once; each answer, or the error text of a child that failed, in order."""
        calls = json.dumps([{"question": question, **kwargs} for question in questions])
        return self.eval("[a if not isinstance(a, BaseException) else f'error: {a}' for a in "
                         f"await __import__('asyncio').gather(*[rlm.ask(**c) for c in _yi_json.loads({calls!r})], "
                         "return_exceptions=True)]")

    def _request(self, method: str, params: dict) -> dict:
        self._serial += 1
        serial = self._serial
        self._proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": serial, "method": method, "params": params}) + "\n")
        self._proc.stdin.flush()
        while True:
            frame = self._frame()
            if frame.get("id") == serial and "method" not in frame:
                if "error" in frame:
                    raise YiError(frame["error"].get("message", str(frame["error"])))
                return frame["result"]
    def _frame(self) -> dict:
        while True:
            line = self._proc.stdout.readline()
            if not line:
                raise YiError(f"yi acp exited with status {self._proc.wait()}")
            frame = json.loads(line)
            if frame.get("method") == "session/request_permission":
                # Invariant: nobody is here to approve, so an ask is refused rather than left to hang the run.
                self._proc.stdin.write(json.dumps({"jsonrpc": "2.0", "id": frame["id"], "result": {
                    "outcome": {"outcome": "selected", "optionId": "reject_once"}}}) + "\n")
                self._proc.stdin.flush()
                continue
            return frame
