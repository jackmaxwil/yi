"""Yi as a harbor installed agent (YI_DESIGN 15.5; contracts in A.12).

Register out of tree:
    PYTHONPATH=evals/adapters harbor run --agent yi_harbor.agent:Yi -d <suite>
"""

import os
from pathlib import Path

from harbor.agents.model_connection import ModelConnectionSpec
from harbor.agents.installed.base import BaseInstalledAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from yi_usage import (
    ADAPTER_VERSION,
    EVENTS_FILENAME,
    config_fingerprint,
    parse_events,
    run_command,
)

REMOTE_BINARY = "/usr/local/bin/yi"
# One static musl binary, one step: dodges pier's 360 s setup cap and keeps the
# network allowlist at the provider host (15.3 lever 8).
BINARY_URL_ENV = "EVAL_BINARY_URL"
BINARY_PATH_ENV = "EVAL_BINARY"
VERSION_CHECK = f"{REMOTE_BINARY} --version"


def install_command(url):
    return (
        f"set -euo pipefail; curl -fsSL {url} -o {REMOTE_BINARY} && "
        f"chmod +x {REMOTE_BINARY} && {VERSION_CHECK}"
    )


class Yi(BaseInstalledAgent):
    # passthrough: Yi reads ANTHROPIC_API_KEY / OPENAI_API_KEY / OPENROUTER_API_KEY
    # under their own names (crates/ai/src/auth.rs), so harbor must not rename them.
    MODEL_CONNECTION = ModelConnectionSpec(passthrough=True)

    @staticmethod
    def name():
        return "yi"

    def get_version_command(self):
        return VERSION_CHECK

    async def install(self, environment: BaseEnvironment) -> None:
        url = os.environ.get(BINARY_URL_ENV)
        if url:
            await self.exec_as_root(environment, command=install_command(url))
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

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        await self.exec_as_agent(
            environment,
            command=run_command(self.model_name, self.render_instruction(instruction)),
            env=dict(self.model_connection.env),
        )

    def populate_context_post_run(self, context: AgentContext) -> None:
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
                self.version(), self.model_name, "yolo", None
            ),
        }
