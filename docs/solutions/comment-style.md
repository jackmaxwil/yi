# Comment style: typed referents and named grants

The law is YI_DESIGN.md §18; the decision is [D55](adr/d55.md). This is the
working recipe — what to type, how the resolver behaves, and what the gate says
when it is wrong.

## What a backtick means

Every backticked token in a comment is one of four things. Only the first is a
link.

| Referent | Write | Real example in the tree |
|---|---|---|
| Rust item | `` [`Name`] `` | `` [`Patcher::prepare`] `` in tools/src/hashline/tool.rs |
| Parameter or local of the documented fn | bare | `` `keep_recent` `` in context/src/cut.rs |
| Serialized wire name | bare | `` `display_data` `` in types/src/kernel.rs |
| Symbol in a reference codebase | bare | `` `convertToLlm` `` in context/src/convert.rs |

Three of the four stay bare, so the link is the signal, not the default. Of 26
identifiers in the tree that matched a definition by name, 9 were one of the
bottom three rows and were correctly left alone — `grep` and `git` are tool
names, `stop` is a string key, `permission` is a parameter.

## Qualify the path

rustdoc resolves a link relative to **the documented item's own module scope**,
not the file. A bare method name almost never resolves, even when the method is
three lines below in the same file. Eight of the seventeen conversions in the
D55 sweep failed on the first doc build for exactly this.

```rust
/// Advice reaches the primary only through [`AdvisorRuntime::deliver_reviewed`].
/// Events are observed in a spawned task, mirroring [`crate::goal::attach_goal`].
/// the tool gate half rides [`crate::tools::ToolAdapter`].
```

Rules of thumb, cheapest first:

- Same `impl` block, documenting a sibling method: `` [`Self::method`] ``.
- Method on a type: qualify with the type — `` [`AgentSession::set_model`] ``.
- Free fn or type in another module: `` [`crate::goal::attach_goal`] ``.
- A module itself is linkable: `` [`crate::app`] ``.
- Items in a dependency resolve too, if the crate is a direct dependency.

Links only resolve in `///` and `//!`. A comment inside a function body cannot
carry one, so items stay bare there. That is the rule's ceiling, not a loophole
to reach for — prefer the doc comment where the choice exists.

## What the gate says

`cargo doc --workspace --no-deps --document-private-items` runs inside
`check_guardrails.sh`. `--document-private-items` is load-bearing: most of Yi's
items are `pub(crate)` or narrower, and rustdoc will not resolve a link into
them without it.

```
error: unresolved link to `deliver_reviewed`
  --> crates/runtime/src/advisor/mod.rs:210:66
```
The name does not resolve from that item's scope. Qualify it — or the item was
renamed and the comment is stale, which is the case this gate exists to catch.
It found one on its first run: `Terminal::set_viewport_height` had been renamed
to `resize_viewport` and the comment never followed.

```
error: public documentation for `reduce` links to private item `never_worse`
```
Should not appear — `private_intra_doc_links` is `allow` in
`[workspace.lints.rustdoc]`, because Yi publishes no crate and `pub` here means
"visible to the next crate up", never docs.rs. If it fires, the lint was
re-armed.

## Named grants

A comment invoking §18's incident or invariant grant says which one on its first
line:

```rust
/// Invariant: catastrophic denylist (every mode, yolo included) > configured
/// deny > session rule > configured allow/ask > hold > mode fallback.
```

The vocabulary is closed. `check_comments.py` rejects anything else:

```
crates/permission/src/decide.rs:88: 'Precedence:' is not a §18 grant (Incident|Invariant)
```

Schema facts — grant (3) — need no tag; `crates/types/` is the tag. Untagged
prose is still allowed and still has to earn its line on content alone. The tag
rides an existing first line, so it costs nothing against the volume ratchet.

Lowercase markers are outside the pattern by design, which is what keeps
`// ponytail: ...` deferral notes working.

## Running it yourself

```bash
cargo doc --workspace --no-deps --document-private-items
python3 scripts/guardrails/check_comments.py
```

Both are inside `just check`. Neither moves a baseline: links and tags replace
text on lines that already exist, so comment volume is unchanged by a conversion.

## Out of scope

Design-doc anchors (§19, D47, U35) and `ref/` line spans are not Rust items.
rustdoc cannot check them and this convention does not cover them; `ref/` spans
are re-verified when a reference is re-cloned, per the Appendix A discipline.
