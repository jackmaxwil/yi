"""Yi as a pier installed agent (evals/README.md).

Same run command and usage parse as the harbor adapter, plus pier's declarative
install spec, its network allowlist, and its three extra context columns.

Register out of tree:
    PYTHONPATH=evals/adapters pier run --agent yi_pier.agent:Yi -d <suite>
"""

import os

from pier.agents.installed.base import BaseInstalledAgent
from pier.environments.base import BaseEnvironment
from pier.models.agent.context import AgentContext
from pier.models.agent.install import AgentInstallSpec, InstallStep
from pier.models.agent.network import NetworkAllowlist

from yi_usage import (
    ADAPTER_VERSION,
    EVENTS_FILENAME,
    SESSIONS_SUBDIR,
    config_fingerprint,
    parse_events,
    run_command,
    session_extras,
)

REMOTE_BINARY = "/usr/local/bin/yi"
BINARY_URL_ENV = "EVAL_BINARY_URL"
VERSION_CHECK = f"{REMOTE_BINARY} --version"
PROVIDER_DOMAINS = {
    "anthropic": "api.anthropic.com",
    "openai": "api.openai.com",
    "openrouter": "openrouter.ai",
}


class Yi(BaseInstalledAgent):
    @staticmethod
    def name():
        return "yi"

    def get_version_command(self):
        return VERSION_CHECK

    def install_spec(self) -> AgentInstallSpec:
        """One step, so pier inlines it into the generated Dockerfile.

        A build-time install is what keeps a static musl binary under pier's
        360 s agent-setup cap (trial.py:177).
        """
        url = os.environ.get(BINARY_URL_ENV)
        if not url:
            raise ValueError(f"set {BINARY_URL_ENV} to the x86_64 musl release URL")
        return AgentInstallSpec(
            agent_name=self.name(),
            version=self.version(),
            steps=[
                InstallStep(
                    user="root",
                    run=(
                        f"set -euo pipefail; curl -fsSL {url} -o {REMOTE_BINARY} && "
                        f"chmod +x {REMOTE_BINARY}"
                    ),
                )
            ],
            verification_command=VERSION_CHECK,
        )

    def network_allowlist(self) -> NetworkAllowlist:
        provider = (self.model_name or "").split("/", 1)[0]
        domain = PROVIDER_DOMAINS.get(provider)
        return NetworkAllowlist(domains=[domain] if domain else [])

    async def run(
        self,
        instruction: str,
        environment: BaseEnvironment,
        context: AgentContext,
    ) -> None:
        await self.exec_as_agent(
            environment,
            command=run_command(self.model_name, self.render_instruction(instruction)),
            env=self.build_process_env(),
        )

    def populate_context_post_run(self, context: AgentContext) -> None:
        usage = parse_events(self.logs_dir / EVENTS_FILENAME)
        cache_read = usage["cacheRead"]
        context.n_input_tokens = (
            None if usage["input"] is None else usage["input"] + cache_read
        )
        context.n_cache_tokens = cache_read
        context.n_output_tokens = usage["output"]
        context.cost_usd = usage["costUsd"]
        extras = session_extras(self.logs_dir / SESSIONS_SUBDIR)
        context.peak_context_tokens = extras["peak_context_tokens"]
        context.summarization_count = extras["summarization_count"]
        context.n_agent_steps = extras["n_agent_steps"]
        context.metadata = {
            "adapterVersion": ADAPTER_VERSION,
            "malformedLines": usage["malformedLines"],
            "configFingerprint": config_fingerprint(
                self.version(), self.model_name, "yolo", None
            ),
        }
