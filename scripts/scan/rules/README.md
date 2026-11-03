# Scanner provenance

Pinned inputs were downloaded on 2026-10-28. Runtime scans use vendored rules and never fetch a registry pack.

| Input | Source URL | SHA-256 |
|---|---|---|
| zizmor 1.31.1 macOS arm64 wheel | https://files.pythonhosted.org/packages/d8/8f/aeb02ac2a769e9b35991633ea04a43481616b2895c82cc5cac58aeeafd0d/zizmor-1.31.1-py3-none-macosx_11_0_arm64.whl | 4a70ee3c2721122aff04d5bbad5e448d15384f45de2ecb3c9493fe9dac2bc95f |
| gitleaks 8.30.1 macOS arm64 archive | https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_darwin_arm64.tar.gz | b40ab0ae55c505963e365f271a8d3846efbc170aa17f2607f13df610a9aeb6a5 |
| gitleaks 8.30.1 Linux x64 archive | https://github.com/gitleaks/gitleaks/releases/download/v8.30.1/gitleaks_8.30.1_linux_x64.tar.gz | 551f6fc83ea457d62a0d98237cbad105af8d557003051f41f3e7ca7b3f2470eb |
| Semgrep 1.182.1 macOS arm64 wheel | https://files.pythonhosted.org/packages/ad/be/e93f291ccda669ae3be90e2716cd812b43d3fa1772f0c4a4f961abe6baad/semgrep-1.182.1-cp310.cp311.cp312.cp313.cp314.py310.py311.py312.py313.py314-none-macosx_11_0_arm64.whl | 89fe0afe240455800f806145f5679707892ea9bbd442a56eab1ea0b25a95f4c6 |
| Semgrep Rules License | https://raw.githubusercontent.com/semgrep/semgrep-rules/develop/LICENSE | 1a6466ae5f1c68631bbd13e3f1aa8f380d84219853c08f0456b853fccfacee1d |
| dangerous-subprocess-use-tainted-env-args.yaml | https://raw.githubusercontent.com/semgrep/semgrep-rules/develop/python/lang/security/audit/dangerous-subprocess-use-tainted-env-args.yaml | 0f3f3f8fafef5b828a15253585e4c6648c46c6fd1cca4b0ae8c70a9399def44d |
| subprocess-shell-true.yaml | https://raw.githubusercontent.com/semgrep/semgrep-rules/develop/python/lang/security/audit/subprocess-shell-true.yaml | 753ba9d29d592b3d7f5086e671e424ee05138acbf75a39b663d92aa3275f5380 |
| dangerous-os-exec-tainted-env-args.yaml | https://raw.githubusercontent.com/semgrep/semgrep-rules/develop/python/lang/security/audit/dangerous-os-exec-tainted-env-args.yaml | 593edc8b93d10cfdc478070dc30a9acc74d27d47d03fa883c0aa9b7351f8e219 |
| insecure-hash-algorithms-md5.yaml | https://raw.githubusercontent.com/semgrep/semgrep-rules/develop/python/lang/security/insecure-hash-algorithms-md5.yaml | e8a898841202c02d986b4931e3f4550fced6056f310e6069bf41720a96937580 |
| insecure-hash-algorithms.yaml | https://raw.githubusercontent.com/semgrep/semgrep-rules/develop/python/lang/security/insecure-hash-algorithms.yaml | 6adf7bcd537f1374955ccbdea68cef115482238f2b11e8d931497cc0afcb859e |

The guardrail invokes `uvx --from zizmor==1.31.1 zizmor` and uses `--offline` outside CI. `uv` resolves the platform wheel; the recorded URL and hash cover the development-machine download. CI may download the pinned release from PyPI because tool installation is the only scanner network access it needs.

Registry files under `semgrep/registry/` retain their upstream IDs and the Semgrep Rules License v1.0, which permits internal use. The five files are the exact Python families found by the 2026-10-28 trial; `p/rust`, `p/github-actions`, `p/bash`, `p/dockerfile` and `p/secrets` are not shipped. Yi rules cover subprocess shell/f-string commands, environment-selected process programs, non-test `temp_dir`, and unchecked UTF-8; workflow event text reaching a `run:` is zizmor's `template-injection` audit, not a Semgrep rule. Existing panic and string-slice gates cover the other proposed Rust constructs.