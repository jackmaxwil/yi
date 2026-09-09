"""Yi as a harbor installed agent (YI_DESIGN 15.5; contracts in A.12).

Register out of tree:
    PYTHONPATH=evals/adapters harbor run --agent yi_harbor.agent:Yi -d <suite>
"""


import json
import os
import sys
from pathlib import Path

from harbor.agents.model_connection import ModelConnectionSpec
from harbor.agents.installed.base import BaseInstalledAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from yi_usage import (
    ADAPTER_VERSION,
    EVENTS_FILENAME,
    SESSIONS_SUBDIR,
    config_fingerprint,
    kernel_problems,
    parse_events,
    run_command,
    TASK_TIMEOUT_SEC,
)

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import atif  # noqa: E402

TRAJECTORY_FILENAME = "trajectory.json"

REMOTE_BINARY = "/usr/local/bin/yi"
# The run's numbers (D132) need telemetry on in the agent user's HOME; the
# adapter writes the one config line at install, never the caller's ~/.yi.
TELEMETRY_CONFIG = '{"telemetry":{"enabled":true}}'
CONFIG_COMMAND = (
    "mkdir -p ~/.yi && printf '%s' " + repr(TELEMETRY_CONFIG) + " > ~/.yi/config.json"
)
# Incident: `oven/bun` ships no CA roots, and the platform verifier (YI_DESIGN
# 13.3: no bundled store) refused OpenRouter's certificate ("UnknownIssuer") on
# the first request; harbor's own certifi bundle rides along and SSL_CERT_FILE
# names it, which rustls-native-certs honours on Linux.
REMOTE_CA_BUNDLE = "/logs/agent/yi/ca.pem"
SUITE_REV_ENV = "EVAL_SUITE_REV"
TIMEOUT_MULT_ENV = "EVAL_TIMEOUT_MULT"
# One static musl binary, one step: dodges pier's 360 s setup cap and keeps the
# network allowlist at the provider host (15.3 lever 8).
BINARY_URL_ENV = "EVAL_BINARY_URL"
BINARY_PATH_ENV = "EVAL_BINARY"
VERSION_CHECK = f"{REMOTE_BINARY} --version"
# The kernel venv is built at install, inside the agent-setup cap and off the hour;
# a v4 image without python3 is an install error here, never a bash-only run.
DOCTOR_COMMAND = f"{REMOTE_BINARY} doctor --fix --json || true"


def mode_label():
    """`yolo`, plus the timeout multiplier when the driver set one: a one-hour
    row and an eight-hour row must never share a fingerprint (plan §4)."""
    mult = os.environ.get(TIMEOUT_MULT_ENV)
    return f"yolo+t{mult}" if mult else "yolo"


def install_command(url):
    return (
        f"set -euo pipefail; curl -fsSL {url} -o {REMOTE_BINARY} && "
        f"chmod +x {REMOTE_BINARY} && {VERSION_CHECK}"
    )


class Yi(BaseInstalledAgent):
    # passthrough: Yi reads ANTHROPIC_API_KEY / OPENAI_API_KEY / OPENROUTER_API_KEY
    # under their own names (crates/ai/src/auth.rs), so harbor must not rename them.
    MODEL_CONNECTION = ModelConnectionSpec(passthrough=True)
    # harbor sets _resume around run() only when this is declared (base.py:965-971).
    SUPPORTS_RESUME = True
    # E10: the trajectory the hub viewer and any judge read (plan S4).
    SUPPORTS_ATIF = True

    @staticmethod
    def name():
        return "yi"

    def get_version_command(self):
        return VERSION_CHECK

    async def install(self, environment: BaseEnvironment) -> None:
        url = os.environ.get(BINARY_URL_ENV)
        if url:
            await self.exec_as_root(environment, command=install_command(url))
            await self.exec_as_agent(environment, command=CONFIG_COMMAND)
            await self.upload_ca_bundle(environment)
            await self.warm_kernel(environment)
            return
        local = os.environ.get(BINARY_PATH_ENV)
        if not local:
            raise ValueError(
                f"set {BINARY_URL_ENV} (release URL) or {BINARY_PATH_ENV} "
                "(local x86_64 musl build) before running the Yi adapter"
            )
        await environment.upload_file(Path(local), REMOTE_BINARY)
        await self.exec_as_root(
            environment,
            command=f"set -euo pipefail; chmod +x {REMOTE_BINARY} && {VERSION_CHECK}",
        )
        await self.exec_as_agent(environment, command=CONFIG_COMMAND)
        await self.upload_ca_bundle(environment)
        await self.warm_kernel(environment)

    async def warm_kernel(self, environment: BaseEnvironment) -> None:
        result = await self.exec_as_agent(environment, command=DOCTOR_COMMAND)
        stdout = getattr(result, "stdout", None)
        if stdout is None:
            return
        problems = kernel_problems(stdout)
        if problems:
            # the trial runs anyway: the environment block tells the model the kernel is
            # unavailable and the row counts `kernel_dead` (#329)
            print("kernel: " + "; ".join(problems), file=sys.stderr)

    async def upload_ca_bundle(self, environment: BaseEnvironment) -> None:
        try:
            import certifi
        except ImportError:
            return
        await self.exec_as_agent(environment, command=f"mkdir -p {Path(REMOTE_CA_BUNDLE).parent}")
        await environment.upload_file(Path(certifi.where()), REMOTE_CA_BUNDLE)

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        await self.exec_as_agent(
            environment,
            command=run_command(
                self.model_name,
                self.render_instruction(instruction),
                resume=self._resume,
                deadline_sec=self.deadline_sec(),
            ),
            env={**self.model_connection.env, "SSL_CERT_FILE": REMOTE_CA_BUNDLE},
        )

    def deadline_sec(self):
        """Harbor never tells the agent its timeout; the driver's multiplier does."""
        return int(TASK_TIMEOUT_SEC * float(os.environ.get(TIMEOUT_MULT_ENV) or 1))

    def write_trajectory(self):
        """The synced session file as ATIF, beside the event stream in logs_dir."""
        sessions = sorted((self.logs_dir / SESSIONS_SUBDIR).rglob("*.jsonl"))
        sessions = [path for path in sessions if not path.name.endswith(".telemetry.jsonl")]
        if not sessions:
            return None
        lines = []
        for path in sessions:
            lines.extend(path.read_text(errors="replace").splitlines())
        trajectory = atif.convert(lines, self.version() or "unknown", self.model_name)
        target = self.logs_dir / TRAJECTORY_FILENAME
        target.write_text(json.dumps(trajectory, indent=2) + "\n")
        return target

    def populate_context_post_run(self, context: AgentContext) -> None:
        self.write_trajectory()
        usage = parse_events(self.logs_dir / EVENTS_FILENAME)
        cache_read = usage["cacheRead"]
        # harbor's convention (pi.py:260): the input column includes cache reads.
        context.n_input_tokens = (
            None if usage["input"] is None else usage["input"] + cache_read
        )
        context.n_cache_tokens = cache_read
        context.n_output_tokens = usage["output"]
        context.cost_usd = usage["costUsd"]
        context.metadata = {
            "adapterVersion": ADAPTER_VERSION,
            "malformedLines": usage["malformedLines"],
            "configFingerprint": config_fingerprint(
                self.version(), self.model_name, mode_label(), os.environ.get(SUITE_REV_ENV)
            ),
            "timeoutMultiplier": os.environ.get(TIMEOUT_MULT_ENV),
        }
