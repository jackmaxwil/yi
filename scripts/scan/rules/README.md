# Scanner provenance

Pinned inputs were downloaded on 2026-10-28. Runtime scans use vendored rules and never fetch a registry pack.

| Input | Source URL | SHA-256 |
|---|---|---|
| zizmor 1.31.1 macOS arm64 wheel | https://files.pythonhosted.org/packages/d8/8f/aeb02ac2a769e9b35991633ea04a43481616b2895c82cc5cac58aeeafd0d/zizmor-1.31.1-py3-none-macosx_11_0_arm64.whl | 4a70ee3c2721122aff04d5bbad5e448d15384f45de2ecb3c9493fe9dac2bc95f |
| gitleaks 8.30.1 macOS arm64 archive | https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_darwin_arm64.tar.gz | b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5 |
| gitleaks 8.30.1 Linux x64 archive | https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz | 551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb |

The guardrail invokes `uvx --from zizmor==1.31.1 zizmor` and uses `--offline` outside CI. `uv` resolves the platform wheel; the recorded URL and hash cover the development-machine download. CI may download the pinned release from PyPI because tool installation is the only scanner network access it needs.
