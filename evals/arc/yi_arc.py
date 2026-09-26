"""Yi playing ARC-AGI-3 through the official arcprize scaffold.

One Yi session per game: the first action opens it, every later action resumes
it with `--continue`, so Yi's own context management carries the game state
across the turn loop. Nothing here is written into the scaffold clone.

    cd ref/benchmarks/ARC-AGI-3-Agents
    ARC_API_KEY=... OPENROUTER_API_KEY=... \
        ./.venv/bin/python <repo>/evals/arc/yi_arc.py --game ls20

Knobs, all read from the environment (never `YI_*`: these are the harness's,
not the binary's): ARC_YI_BINARY, ARC_YI_MODEL, ARC_MAX_ACTIONS,
ARC_COST_CAP, ARC_RUN_DIR.
"""

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

REPO = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(REPO / "evals" / "adapters"))

from yi_usage import config_fingerprint, parse_events, session_extras  # noqa: E402

BINARY = os.environ.get("ARC_YI_BINARY", str(REPO / "target" / "debug" / "yi"))
MODEL = os.environ.get("ARC_YI_MODEL", "openrouter/z-ai/glm-5.3-flash")
ACTION_BUDGET = int(os.environ.get("ARC_MAX_ACTIONS", "60"))
COST_CAP = float(os.environ.get("ARC_COST_CAP", "10.0"))
TURN_TIMEOUT = int(os.environ.get("ARC_TURN_TIMEOUT", "300"))

BRIEFING = """You are playing an ARC-AGI-3 puzzle game called `{game}`. Nobody has
told you the rules; discovering them by experiment IS the task.

Each turn I give you the board as a {h}x{w} grid of hex digits, one row per
line, where each digit is a colour index (0-f). You reply with one action.

This game accepts exactly these actions and no others: {names}. Any other name
is rejected and the turn is wasted, so never invent one — the `available` list
on each turn is the whole menu. RESET restarts the current level.{clicknote}

The game has {levels} levels. A level is cleared by reaching a goal state you
must infer from how the board reacts to your actions. You have {budget} actions
total, so spend early ones probing what each action does and say what you
learned in `why` — you will see your own earlier replies on later turns, and
they are the only notes you get.

Reply with JSON only: {{"action": "<name>", "why": "<one line>"}}."""

CLICK_NOTE = """ ACTION6 is a click and also needs `x` and `y` in 0..{maxxy}."""


def render(grid):
    return "\n".join("".join(format(v, "x") if 0 <= v < 16 else "?" for v in row) for row in grid)


def changed_cells(previous, grid):
    if previous is None or len(previous) != len(grid):
        return None
    return sum(
        1
        for a, b in zip(previous, grid)
        for x, y in zip(a, b)
        if x != y
    )


class YiRunner:
    """Drives one `yi ask` process per action against one resumed session."""

    def __init__(self, run_dir, game_id):
        self.turn_dir = Path(run_dir) / "turns"
        self.turn_dir.mkdir(parents=True, exist_ok=True)
        self.work = Path(run_dir) / "work"
        self.work.mkdir(parents=True, exist_ok=True)
        self.sessions = Path(run_dir) / "sessions" / game_id
        self.game_id = game_id
        self.turns = 0
        self.retries = 0
        self.cost = 0.0
        self.tokens = {"input": 0, "output": 0, "cacheRead": 0, "cacheWrite": 0}
        self.unknown_usage = 0
        self.started = False
        self.last_error = ""

    def _account(self, path):
        parsed = parse_events(path)
        for key in self.tokens:
            value = parsed.get(key)
            if isinstance(value, (int, float)):
                self.tokens[key] += value
        self.unknown_usage += parsed.get("costUnknownTurns", 0)
        if isinstance(parsed.get("costUsd"), float):
            self.cost += parsed["costUsd"]
        return parsed

    def ask(self, prompt, schema):
        """Return the parsed JSON answer, or None if the turn did not produce one.

        `--schema` rejects a bad answer with the reason; re-asking blind just
        buys the same answer again, so a retry carries the validator's own
        complaint and the allowed names back to the model."""
        allowed = ", ".join(schema["properties"]["action"].get("enum", []))
        for attempt in range(3):
            self.turns += 1
            if attempt:
                self.retries += 1
                prompt = (
                    f"That reply was rejected: {self.last_error}\n\n"
                    f"The only action names this game accepts are: {allowed}. "
                    "Pick one of those and reply with JSON only."
                )
            path = self.turn_dir / f"{self.game_id}.{self.turns:04d}.jsonl"
            command = [
                BINARY, "ask", "--json", "--yolo",
                "--model", MODEL,
                "--cwd", str(self.work),
                "--session-dir", str(self.sessions),
                "--schema", json.dumps(schema),
            ]
            if self.started:
                command.append("--continue")
            command.append(prompt)
            try:
                done = subprocess.run(
                    command, capture_output=True, text=True, timeout=TURN_TIMEOUT
                )
            except subprocess.TimeoutExpired:
                path.write_text("")
                print(f"  yi timed out after {TURN_TIMEOUT}s", file=sys.stderr)
                return None
            path.write_text(done.stdout)
            self.started = True
            self._account(path)
            answer = last_assistant_text(path)
            value = extract_json(answer)
            if done.returncode == 0 and isinstance(value, dict):
                return value
            self.last_error = (
                done.stderr.strip().splitlines()[-1][:300]
                if done.stderr.strip()
                else f"yi exited {done.returncode} with no JSON answer"
            )
            print(f"  yi exit {done.returncode}: {self.last_error}", file=sys.stderr)
        return None


def last_assistant_text(path):
    text = ""
    for line in Path(path).read_text(errors="replace").splitlines():
        line = line.strip()
        if not line:
            continue
        try:
            event = json.loads(line)
        except json.JSONDecodeError:
            continue
        if not isinstance(event, dict) or event.get("type") != "message_end":
            continue
        message = event.get("message")
        if not isinstance(message, dict) or message.get("role") != "assistant":
            continue
        chunks = [
            block.get("text", "")
            for block in message.get("content") or []
            if isinstance(block, dict) and block.get("type") == "text"
        ]
        if chunks:
            text = "".join(chunks)
    return text


def extract_json(text):
    start, end = text.find("{"), text.rfind("}")
    if start < 0 or end <= start:
        return None
    try:
        return json.loads(text[start : end + 1])
    except json.JSONDecodeError:
        return None


def build_agent_class():
    from arcengine import FrameData, GameAction, GameState

    from agents.agent import Agent

    class Yi(Agent):
        MAX_ACTIONS = ACTION_BUDGET

        def __init__(self, *args, **kwargs):
            super().__init__(*args, **kwargs)
            self.runner = YiRunner(RUN_DIR, self.game_id)
            self.previous = None
            self.briefed = False
            self.stop_reason = None
            self.level_actions = {}
            self.model_actions = 0
            self.step_retries = 0
            no_keepalive(getattr(self.arc_env, "_session", None))

        def do_action_request(self, action):
            """A Yi turn takes tens of seconds, so the ARC socket idles between
            actions; `step` swallows the dropped connection and returns None,
            which the base class then raises on. Retry rather than die."""
            for _ in range(3):
                try:
                    return super().do_action_request(action)
                except ValueError:
                    self.step_retries += 1
                    time.sleep(2)
            self.stop_reason = "arc-step-failed"
            raise RuntimeError(f"{self.game_id}: three ARC steps in a row returned no frame")

        @property
        def name(self):
            return f"{self.game_id}.yi.{self.MAX_ACTIONS}"

        def is_done(self, frames, latest_frame):
            if self.stop_reason:
                return True
            if latest_frame.state is GameState.WIN:
                self.stop_reason = "win"
                return True
            if self.runner.cost >= COST_CAP:
                self.stop_reason = "cost-cap"
                return True
            return False

        def choose_action(self, frames, latest_frame):
            level = latest_frame.levels_completed or 0
            self.level_actions[level] = self.level_actions.get(level, 0) + 1
            if latest_frame.state in (GameState.NOT_PLAYED, GameState.GAME_OVER):
                action = GameAction.RESET
                action.reasoning = {"why": f"auto-reset from {latest_frame.state.name}"}
                self.previous = None
                return action
            grid = latest_frame.frame[-1] if latest_frame.frame else []
            names = [GameAction.from_id(i).name for i in (latest_frame.available_actions or [])]
            names = [n for n in names if n != "RESET"] + ["RESET"]
            schema = {
                "type": "object",
                "required": ["action"],
                "properties": {
                    "action": {"type": "string", "enum": names},
                    "x": {"type": "integer"},
                    "y": {"type": "integer"},
                    "why": {"type": "string"},
                },
            }
            prompt = self.compose(latest_frame, grid, names)
            self.previous = grid
            answer = self.runner.ask(prompt, schema)
            if answer is None:
                self.stop_reason = "yi-turn-failed"
                return GameAction.RESET
            self.model_actions += 1
            try:
                action = GameAction[answer["action"]]
            except KeyError:
                self.stop_reason = f"unknown action {answer.get('action')!r}"
                return GameAction.RESET
            if action.is_complex():
                action.set_data(
                    {
                        "x": max(0, min(63, int(answer.get("x", 32)))),
                        "y": max(0, min(63, int(answer.get("y", 32)))),
                    }
                )
            action.reasoning = {"why": str(answer.get("why", ""))[:400]}
            return action

        def compose(self, frame, grid, names):
            head = []
            if not self.briefed:
                self.briefed = True
                maxxy = (len(grid[0]) - 1) if grid else 63
                head.append(
                    BRIEFING.format(
                        game=self.game_id,
                        h=len(grid),
                        w=len(grid[0]) if grid else 0,
                        names=", ".join(names),
                        clicknote=CLICK_NOTE.format(maxxy=maxxy)
                        if "ACTION6" in names
                        else "",
                        levels=frame.win_levels,
                        budget=ACTION_BUDGET,
                    )
                )
            delta = changed_cells(self.previous, grid)
            head.append(
                f"Action {self.action_counter + 1}/{self.MAX_ACTIONS}. "
                f"state={frame.state.name} levels_completed={frame.levels_completed}"
                f"/{frame.win_levels} available={','.join(names)}"
                + ("" if delta is None else f" cells_changed_by_last_action={delta}")
            )
            head.append("Board:\n" + render(grid))
            return "\n\n".join(head)

    return Yi


def yi_version():
    try:
        done = subprocess.run([BINARY, "version"], capture_output=True, text=True, timeout=30)
        return done.stdout.strip() or "unknown"
    except (OSError, subprocess.SubprocessError):
        return "unknown"


def scaffold_rev(path):
    try:
        done = subprocess.run(
            ["git", "-C", str(path), "rev-parse", "--short", "HEAD"],
            capture_output=True,
            text=True,
            timeout=30,
        )
        return done.stdout.strip() or "unknown"
    except (OSError, subprocess.SubprocessError):
        return "unknown"


def no_keepalive(session):
    """Idle gaps between Yi turns outlive the ARC server's keep-alive window, and
    a reused dead socket fails the next POST. A fresh connection per request costs
    a TLS handshake against a turn that already took tens of seconds."""
    if session is not None:
        session.headers["Connection"] = "close"


def find_scaffold():
    """ref/ is gitignored, so a worktree has none — walk up to the real checkout."""
    tail = Path("ref") / "benchmarks" / "ARC-AGI-3-Agents"
    for base in (REPO, *REPO.parents):
        if (base / tail / "agents").is_dir():
            return base / tail
    return None


def rhae(run):
    """(human actions / agent actions)^2, capped 1.15, zero for a level not cleared."""
    scores = run.get("level_scores") or []
    actions = run.get("level_actions") or []
    baseline = run.get("level_baseline_actions") or []
    out = []
    for i, base in enumerate(baseline):
        got = actions[i] if i < len(actions) else 0
        cleared = bool(i < len(scores) and scores[i])
        out.append(
            round(min(1.15, (base / got) ** 2), 4) if cleared and got and base else 0.0
        )
    return out


def main():
    parser = argparse.ArgumentParser(description="Yi on ARC-AGI-3")
    parser.add_argument("--game", required=True, help="game id prefix, e.g. ls20")
    parser.add_argument("--tags", default="yi")
    args = parser.parse_args()

    scaffold = Path(os.environ.get("ARC_SCAFFOLD", "")) if os.environ.get(
        "ARC_SCAFFOLD"
    ) else find_scaffold()
    if scaffold is None or not (scaffold / "agents").is_dir():
        sys.exit(f"scaffold not found at {scaffold}; set ARC_SCAFFOLD")
    scaffold = scaffold.resolve()
    sys.path.insert(0, str(scaffold))
    os.chdir(scaffold)

    from dotenv import load_dotenv

    load_dotenv(dotenv_path=".env.example")
    if not os.environ.get("ARC_API_KEY", "").strip() or os.environ[
        "ARC_API_KEY"
    ].startswith("your_"):
        sys.exit("ARC_API_KEY is not set in the environment")
    if not os.environ.get("OPENROUTER_API_KEY", "").strip():
        sys.exit("OPENROUTER_API_KEY is not set in the environment")
    if not Path(BINARY).is_file():
        sys.exit(f"{BINARY} is missing; cargo build -p yi-cli")

    import logging

    logging.getLogger().setLevel(logging.INFO)
    logging.getLogger().addHandler(logging.StreamHandler(sys.stderr))
    logging.getLogger().addHandler(logging.FileHandler(Path(RUN_DIR) / "run.log", mode="w"))

    import agents

    Yi = build_agent_class()
    agents.AVAILABLE_AGENTS["yi"] = Yi

    scheme = os.environ.get("SCHEME", "https")
    host = os.environ.get("HOST", "arcprize.org")
    port = str(os.environ.get("PORT", "443"))
    root = f"{scheme}://{host}" if port in ("80", "443") else f"{scheme}://{host}:{port}"

    import requests

    games = requests.get(
        f"{root}/api/games",
        headers={"X-API-Key": os.environ["ARC_API_KEY"], "Accept": "application/json"},
        timeout=20,
    ).json()
    matched = [g["game_id"] for g in games if g["game_id"].startswith(args.game)]
    if not matched:
        sys.exit(f"no game matching {args.game!r}")

    class Swarm(agents.Swarm):
        def close_scorecard(self, card_id):
            """The only call that turns a paid run into a number; a dropped
            socket here would discard the whole run's score."""
            for attempt in range(3):
                try:
                    return super().close_scorecard(card_id)
                except Exception as error:  # noqa: BLE001
                    print(f"close_scorecard retry {attempt + 1}: {error}", file=sys.stderr)
                    time.sleep(3)
            return None

    started = time.time()
    swarm = Swarm("yi", root, matched[:1], tags=args.tags.split(","))
    no_keepalive(getattr(swarm._arc, "_session", None))
    scorecard = swarm.main()
    wall = round(time.time() - started, 1)

    agent = swarm.agents[0]
    card = scorecard.model_dump() if scorecard else {}
    runs = (card.get("environments") or [{}])[0].get("runs") or [{}]
    suite = f"arc-agi-3@{scaffold_rev(scaffold)}"
    summary = {
        "game": matched[0],
        "model": MODEL,
        "maxActions": ACTION_BUDGET,
        "suiteAtRev": suite,
        "configFp": config_fingerprint(yi_version(), MODEL, "yolo", suite),
        "cardId": card.get("card_id"),
        "scorecardUrl": f"{root}/scorecards/{card.get('card_id')}",
        "levelsCompleted": card.get("total_levels_completed"),
        "totalLevels": card.get("total_levels"),
        "actions": card.get("total_actions"),
        "modelActions": agent.model_actions,
        "levelActions": runs[0].get("level_actions"),
        "levelBaselineActions": runs[0].get("level_baseline_actions"),
        "rhaePerLevel": rhae(runs[0]),
        "stopReason": agent.stop_reason,
        "yiTurns": agent.runner.turns,
        "schemaRetries": agent.runner.retries,
        "arcStepRetries": agent.step_retries,
        "tokens": agent.runner.tokens,
        "costUsd": round(agent.runner.cost, 6),
        "costUnknownTurns": agent.runner.unknown_usage,
        "wallSeconds": wall,
        "session": session_extras(agent.runner.sessions),
        "recording": getattr(agent.recorder, "filename", None),
        "scorecard": card,
    }
    out = Path(RUN_DIR) / "summary.json"
    out.write_text(json.dumps(summary, indent=2))
    trimmed = {k: v for k, v in summary.items() if k != "scorecard"}
    print(json.dumps(trimmed, indent=2))
    print(f"\nsummary: {out}", file=sys.stderr)


RUN_DIR = Path(
    os.environ.get("ARC_RUN_DIR", REPO / "evals" / "arc" / "runs" / time.strftime("%Y%m%dT%H%M%S"))
)

def selftest():
    assert render([[0, 10, 15], [1, 2, 3]]) == "0af\n123"
    assert changed_cells(None, [[1]]) is None
    assert changed_cells([[1, 2], [3, 4]], [[1, 9], [3, 4]]) == 1
    assert extract_json('```json\n{"action": "ACTION1"}\n```') == {"action": "ACTION1"}
    assert extract_json("no json here") is None
    assert extract_json('{"a": ') is None
    # A level cleared under baseline caps at 1.15; over baseline decays; uncleared is 0.
    assert rhae(
        {
            "level_scores": [1.0, 1.0, 0.0],
            "level_actions": [10, 200, 5],
            "level_baseline_actions": [22, 100, 73],
        }
    ) == [1.15, 0.25, 0.0]
    # A run that never reported usage must not price as zero-cost success.
    import tempfile

    with tempfile.NamedTemporaryFile("w", suffix=".jsonl", delete=False) as handle:
        handle.write(
            json.dumps(
                {
                    "type": "message_end",
                    "message": {
                        "role": "assistant",
                        "content": [{"type": "text", "text": '{"action":"ACTION3"}'}],
                        "usage": {"input": 5, "output": 1, "unknown": True},
                    },
                }
            )
            + "\n"
        )
        path = handle.name
    assert last_assistant_text(path) == '{"action":"ACTION3"}'
    assert parse_events(path)["costUsd"] is None
    assert parse_events(path)["costUnknownTurns"] == 1
    os.unlink(path)
    print("selftest ok")


if __name__ == "__main__":
    if "--selftest" in sys.argv:
        selftest()
    else:
        RUN_DIR.mkdir(parents=True, exist_ok=True)
        main()
