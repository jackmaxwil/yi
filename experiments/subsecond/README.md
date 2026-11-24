# subsecond hot-patch experiment

Optional and unwired. This is a standalone Cargo project, excluded from the Yi workspace
(`exclude = ["experiments"]` in the root `Cargo.toml`) and from every gate. It exists to try
Dioxus' `subsecond` runtime hot-patching on a loop shaped like Yi's, nothing more.

Needs the Dioxus CLI (`cargo install dioxus-cli --version 0.8.0-alpha.3`) and the stable
toolchain. From this directory:

    dx serve --hot-patch --windows false

Edit the string in `greeting` and save. The running process prints the new text on the next tick
without restarting. `subsecond::call` marks the patch point, so only code reached through it is
patched.

Not checked here: the `dx` flags above were not run in this lane and may differ in the alpha
release; `cargo check --manifest-path experiments/subsecond/Cargo.toml` is the only thing verified.

Why it is not in the build: the patcher relinks changed functions into a dylib and needs the
Dioxus CLI as the linker driver. It does not combine with the Cranelift `just dev-fast` build, and
it patches one binary at a time, so it cannot replace the workspace build.
