Compaction is prefix-aligned: the stable head of the conversation stays
byte-identical across turns, so the provider's prompt cache keeps hitting
instead of re-reading the whole session every turn.

## Token efficiency

The fixed prefix every request pays — system block plus tool table — is
measured by a test and ratcheted in CI. Adding a tool or a system sentence
fails the build until the growth is committed on purpose.

## Speed

- `yi --version`: ≤ 5 ms budgeted, measured on the dist binary. It returns
  before config parse or runtime construction.
- The async runtime is a current-thread tokio, built per command. No thread
  pool warms up to print a version string.
- Dist binary budget 7 MiB. Every dependency added logs its measured size and
  startup delta in `docs/size-ledger.md` before it lands.

## Memory

Nothing heavy exists until used. The Jupyter kernel compiles into every
build but boots lazily on the first `ipython` call — no Python process
otherwise. MCP is compiled in but runtime-gated off by default. Solo paints an
inline viewport on the normal screen: finished output is written once to
native scrollback and never repainted; a workspace pane paints the same chat
into its rectangle from the retained transcript.

Sessions live on disk as an append-only entry tree, not in RAM: branch,
rewind to any entry, resume after a crash, read with tools that are not this
program. Unknown fields survive round-trips wherever the shape allows
(`docs/YI_DESIGN.md` §20).

## Planning

Planned work lives on the forge, not in this tree: an issue is the identity of
a piece of work, and its number is what everything else cites. A feature pull
request names its issue and the merge closes it — nothing is marked done by
hand. Milestone dates are not typed; they are divided out of measured
throughput and rewritten every week.
