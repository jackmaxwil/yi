---
issue: Closes #1202
raise: crate runtime +37, tests +60, comments +2
---
The kernel's environment was fixed when it booted, so a plugin installed by `yi plugin add` mid-session reached the bash tool and `rlm.bash` (which build PATH per command) but not `!` lines, `%%bash` or `subprocess` in a cell. Before each cell the kernel service now recomputes PATH from the same installed-plugin list the bash tool uses and, when it differs from what the kernel holds, sets `os.environ["PATH"]` through an internal cell. A kernel booted with no plugins and a home that still has none is left alone. The journey test `kernel_lane::a_plugin_added_after_the_kernel_booted_is_on_the_next_cells_path` boots a kernel, publishes a plugin program, and finds it with `!which` and `subprocess`.
