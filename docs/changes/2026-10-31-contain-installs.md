---
version: 0.873.0
decisions: D529
issue: Closes #1932
decision: a package install into the tree runs contained and unasked in auto; a global one, a publish and brew or gem still ask and leave | every install asked or failed now that the proxy gives them a network | restore the classify and host_need arms
raise: tests +57, comments +5, crate kernel +1, crate permission +37, crate runtime +8
---
`cargo add`, `cargo fetch`, `npm`, `pnpm`, `yarn`, `pip`, `uv` and `bun` installs now run contained in `auto` with no question, and in `ask` an approval stays contained. A global install (`-g`, `--user`, `--target`, `--prefix`, `--root`), `cargo install`, `publish`, `login`, `link`, `brew` and `gem` still ask, and an approval runs outside.

`%pip install` works in a kernel. The kernel venv now carries pip, about 6.5 MB, and a cell's install goes to the session's own `site` directory, which is on its path. Existing venvs rebuild once, since the bootstrap schema moved. The prompts stop telling a model to ask before installing.
