"""Yi as a harbor installed agent (evals/README.md).

Register out of tree:
    PYTHONPATH=evals/adapters harbor run --agent yi_harbor.agent:Yi -d <suite>
"""


import hashlib
import json
import os
import shlex
import subprocess
import sys
import tarfile
import tempfile
from pathlib import Path

from harbor.agents.model_connection import ModelConnectionSpec
from harbor.agents.installed.base import BaseInstalledAgent
from harbor.environments.base import BaseEnvironment
from harbor.models.agent.context import AgentContext

from yi_usage import (
    ADAPTER_VERSION,
    EVENTS_FILENAME,
    LEVERS_ENV,
    REMOTE_LEVERS_PATH,
    SESSIONS_SUBDIR,
    config_fingerprint,
    eval_config,
    kernel_broken,
    kernel_problems,
    levers_label,
    local_plugins,
    parse_events,
    plugins_label,
    routing_label,
    run_command,
    TASK_TIMEOUT_SEC,
    with_budget,
)

sys.path.insert(0, str(Path(__file__).resolve().parents[2]))
import atif  # noqa: E402

TRAJECTORY_FILENAME = "trajectory.json"

REMOTE_BINARY = "/usr/local/bin/yi"
# Incident: `oven/bun` ships no CA roots, and the platform verifier (YI_DESIGN
# §18.3: no bundled store) refused OpenRouter's certificate ("UnknownIssuer") on
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
# Incident (#1315): the binary embeds only the kernel's own skills, and `yi update` installs
# `skills/yi`, so every trial before ran with no bundled skill; the arm's own tree rides along.
SKILLS_ENV = "EVAL_SKILLS"
REMOTE_SKILLS_TAR = "/logs/agent/yi/skills.tgz"


def bundled_skills():
    """`EVAL_SKILLS`, else this checkout's `skills/yi`: the tree a real install of the binary has."""
    named = os.environ.get(SKILLS_ENV)
    path = Path(named) if named else Path(__file__).resolve().parents[3] / "skills" / "yi"
    return path if path.is_dir() else None


def skills_label():
    """Two arms with different skill trees never share a fingerprint."""
    path = bundled_skills()
    if path is None:
        return "+skills{}"
    digest = hashlib.sha256()
    for file in sorted(p for p in path.rglob("*") if p.is_file() and "__pycache__" not in p.parts):
        digest.update(str(file.relative_to(path)).encode())
        digest.update(file.read_bytes())
    return "+skills{" + digest.hexdigest()[:12] + "}"


def mode_label():
    """`yolo`, plus the timeout multiplier, the routing and the levers when the driver set
    them: a one-hour row and an eight-hour row, two routings, or a levers trial and the
    defaults must never share a fingerprint (plan §4, E16)."""
    mult = os.environ.get(TIMEOUT_MULT_ENV)
    return ("yolo" + (f"+t{mult}" if mult else "") + routing_label(os.environ)
            + levers_label(os.environ) + skills_label() + plugins_label(carried_plugins()))


def carried_plugins():
    """#1041: path mode carries the plugins `yi plugin add` installed on this host, as the
    binary itself reports them. URL mode, and the pier adapter which installs by URL, carry
    none: there is no host build to ask, and the fingerprint says `+plugins{}`."""
    if os.environ.get(BINARY_URL_ENV):
        return []
    return local_plugins(os.environ, plugin_report())


def local_binary():
    """The host's yi build that path mode uploads and asks for its plugins."""
    binary = os.environ.get(BINARY_PATH_ENV)
    if not binary:
        raise ValueError(
            f"set {BINARY_URL_ENV} (release URL) or {BINARY_PATH_ENV} "
            "(local x86_64 musl build) before running the Yi adapter"
        )
    return binary


def plugin_report():
    """`yi plugin list` is the one reader of the layout `yi plugin add` writes, so the
    adapter carries what yi itself reports, never a second copy of the layout."""
    binary = local_binary()
    result = subprocess.run([binary, "plugin", "list", "--json"], capture_output=True, text=True)
    if result.returncode != 0:
        raise ValueError(
            f"{binary} plugin list exited {result.returncode}: {result.stderr.strip()}"
        )
    return result.stdout


def config_command():
    """The run's numbers (D132) need telemetry on in the agent user's HOME; the
    adapter writes the config at install, never the caller's ~/.yi."""
    config = shlex.quote(json.dumps(eval_config(os.environ)))
    return f"mkdir -p ~/.yi && printf '%s' {config} > ~/.yi/config.json"


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
            await self.exec_as_agent(environment, command=config_command())
            await self.upload_ca_bundle(environment)
            await self.upload_levers(environment)
            await self.upload_skills(environment)
            await self.warm_kernel(environment)
            return
        await environment.upload_file(Path(local_binary()), REMOTE_BINARY)
        await self.exec_as_root(
            environment,
            command=f"set -euo pipefail; chmod +x {REMOTE_BINARY} && {VERSION_CHECK}",
        )
        await self.upload_plugins(environment)
        await self.exec_as_agent(environment, command=config_command())
        await self.upload_ca_bundle(environment)
        await self.upload_levers(environment)
        await self.upload_skills(environment)
        await self.warm_kernel(environment)

    async def upload_plugins(self, environment: BaseEnvironment) -> None:
        """Each carried plugin lands on PATH, where yi finds it, and must answer `--version`
        there: a binary the image cannot run fails the install rather than a silent arm."""
        for name, version, path in carried_plugins():
            remote = f"/usr/local/bin/{name}"
            await environment.upload_file(path, remote)
            await self.exec_as_root(
                environment, command=f"set -euo pipefail; chmod +x {shlex.quote(remote)} && {shlex.quote(remote)} --version"
            )
            print(f"plugin: {name} {version} at {remote}", file=sys.stderr)

    async def upload_levers(self, environment: BaseEnvironment) -> None:
        """E16: the host's YI_LEVERS file rides into the container; `run` asks for it under --eval."""
        local = os.environ.get(LEVERS_ENV)
        if not local:
            return
        await self.exec_as_agent(environment, command=f"mkdir -p {Path(REMOTE_LEVERS_PATH).parent}")
        await environment.upload_file(Path(local), REMOTE_LEVERS_PATH)

    async def upload_skills(self, environment: BaseEnvironment) -> None:
        local = bundled_skills()
        if local is None:
            print("skills: no bundled tree; the trial runs with none", file=sys.stderr)
            return
        with tempfile.TemporaryDirectory() as scratch:
            archive = Path(scratch, "skills.tgz")
            with tarfile.open(archive, "w:gz") as tar:
                tar.add(local, arcname="yi", filter=lambda info: None if "__pycache__" in info.name else info)
            await self.exec_as_agent(environment, command=f"mkdir -p {Path(REMOTE_SKILLS_TAR).parent} ~/.yi/skills")
            await environment.upload_file(archive, REMOTE_SKILLS_TAR)
        await self.exec_as_agent(
            environment, command=f"set -euo pipefail; tar -xzf {REMOTE_SKILLS_TAR} -C ~/.yi/skills"
        )

    async def warm_kernel(self, environment: BaseEnvironment) -> None:
        result = await self.exec_as_agent(environment, command=DOCTOR_COMMAND)
        stdout = getattr(result, "stdout", None)
        if stdout is None:
            return
        problems = kernel_problems(stdout)
        if kernel_broken(problems):
            raise RuntimeError("kernel preflight red with its toolchain present, so the trial "
                               "would score a broken install: " + "; ".join(problems))
        if problems:
            # no toolchain: the trial runs bash-only, the environment block tells the model the
            # kernel is unavailable and the row counts `kernel_dead` (#329)
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
                levers=bool(os.environ.get(LEVERS_ENV)),
            ),
            env={**self.model_connection.env, "SSL_CERT_FILE": REMOTE_CA_BUNDLE},
        )

    def deadline_sec(self):
        """Harbor never tells the agent its timeout; the driver's multiplier does."""
        return int(TASK_TIMEOUT_SEC * float(os.environ.get(TIMEOUT_MULT_ENV) or 1))

    def render_instruction(self, instruction: str) -> str:
        """The task's own last line names all 28800 seconds; the trial gets the multiplier's share."""
        return with_budget(super().render_instruction(instruction), self.deadline_sec())

    def write_trajectory(self):
        """The synced session file as ATIF, beside the event stream in logs_dir."""
        sessions = sorted((self.logs_dir / SESSIONS_SUBDIR).rglob("*.jsonl"))
        sessions = [path for path in sessions if not path.name.endswith(".telemetry.jsonl")]
        if not sessions:
            return None
        lines = []
        for path in sessions:
            lines.extend(path.read_text(errors="replace").split("\n"))
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
            "byUpstream": usage["byUpstream"],
        }
