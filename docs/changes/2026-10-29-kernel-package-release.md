---
version: 0.992.0
decisions: D572
issue: Closes #1546
growth: +271 the review fixes reuse the venv, probe and skill installers, restore the existing tool and MCP contracts, and add focused regression coverage; no new subsystem was added
decision: the kernel package ships no harness store: rlm.harness, rlm.get_harness_state and the Harness* types are deleted with the harness.save_global host request and the RLM_GLOBAL_HARNESS_STATE_DIR and RLM_GLOBAL_HARNESS_HOST env (retires D270) | no prompt, skill or Yi code reads the store, and every line of it rebuilt the venv when it changed | restore python/yi_runtime/src/rlm/harness.py, its re-exports and kernel_state::register_harness_save from git
ledger+: kernel + ipython + rlm() subagents | since the kernel-package release the kernel's environments are two layers: a dependency venv named by the bootstrap schema, the ipykernel and dill pins, the extras and the dependency arrays of the yi_runtime and skill pyprojects, and an overlay per runtime identity inside it whose python the kernel runs, so a package edit installs an overlay and never rebuilds the dependencies a live session imports from, and a named `YI_KERNEL_PYTHON` is probed once per unchanged interpreter (`bootstrap::a_new_runtime_identity_leaves_the_running_one_and_the_dependencies_alone`, `bootstrap::a_named_kernel_python_is_probed_once_until_it_changes`, T0)
raise: crate kernel +347, tests +66, comments +7
---
The kernel venv is now two layers. The dependency venv (`~/.yi/kernel-venv-<hash>`) is named by the bootstrap schema, the ipykernel and dill pins, the default extras and the dependency arrays of the yi_runtime and skill pyprojects; it holds ipykernel, dill and the extras. The yi_runtime package and the Python skills install `--no-deps` into an overlay, `<venv>/overlays/rt-<identity>`, which reads the dependency venv's site-packages through a `.pth` line, and the kernel runs the overlay's python. A change to a `.py` file therefore installs one new overlay in about a second instead of deleting and rebuilding the whole venv, and it never deletes the environment a running session imports from. Old overlays and old dependency venvs are swept only once they are a day settled, hold no bootstrap lock and no running process names them. A `YI_KERNEL_PYTHON` keeps a ready stamp under `~/.yi`, so a second boot over an unchanged interpreter runs no probes. Naming and the sweep live in `crates/kernel/src/venv.rs`.

The kernel's `rlm.harness` store is gone: `rlm.harness`, `rlm.get_harness_state` and the `HarnessEntry`, `HarnessScope`, `HarnessState` and `RefinementEvent` types are deleted, along with the host's `harness.save_global` request and the `RLM_GLOBAL_HARNESS_STATE_DIR` and `RLM_GLOBAL_HARNESS_HOST` kernel environment. The ready check now asserts `rlm` has no `harness`.

## Seen red

- `test_a_value_reads_only_the_keys_it_received`: `AttributeError not raised` when `total_lines` silently resolved the `totalLines` key.
- `test_search_rows_keep_the_signature_and_whole_description`: the row was `{'name': 'fetch', 'summary': 'Read a URL'}` instead of the signature and full description.
- `test_a_result_by_child_id_digests_the_wait_by_session_name`: `RuntimeError: child sub-1 (sub-1) is no longer registered with the parent` with a wait reply keyed by session name `n`, the host's wait-map shape.
- `test_a_result_by_child_id_is_not_told_a_siblings_state`: `RuntimeError: child sub-1 is stuck (repeat break)` when the one child in the wait reply was a sibling, against the single-candidate guess this change deletes.
- `test_mcp_public_names_remain_importable`: `AttributeError: module 'rlm' has no attribute 'McpToolError'` against the public names shipped before this release.
- `a_new_runtime_identity_leaves_the_running_one_and_the_dependencies_alone`: `left: 4`, `right: 12` for missing-source warnings from one dependency venv and two overlays, using the existing fake uv fixture.
