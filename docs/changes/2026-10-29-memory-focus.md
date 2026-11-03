---
issue: Closes #1860
raise: comments +9, crate runtime +509, tests +363
growth: +509 the ranking reuses `memory::rank`'s BM25, `store::loaded`'s rows and `listed::cut_row`; a fourth module (`memory/focus.rs`) was weighed and its 40 lines stayed in `ext.rs` instead, and the 120-line test block trades against no deletion because every case is a contract the eval holds end to end
decision: the memory block is re-rendered once, at the first typed prompt, ranked by that prompt's words with type weights and near-duplicate hooks dropped, its bodies inside the byte budget the index it replaces already spent | every session carried the whole index, so the model paid 335 tokens of a stored-note map for a request no note addressed, and no body ever reached it | restore `MemoryExt::interests` to `SESSION_START` and delete the `PromptSubmitted` arm, which brings back exactly the pre-change block (that revert is the test's `Seen red` run)
ledger+: host-written memory (one file per fact, hook index) | the first typed prompt re-renders the block ranked by its own words (`memory::rank`'s BM25), with `feedback` and `project` weighted over `reference`, a note that restates another's hook keeping its row but earning no body, the bodies of at most three notes inside the index's byte budget (never under two body caps), and every cut named; a blank first prompt and a resumed session leave the block as it stood (`memory::ext::tests::the_prompt_earns_the_body_of_the_note_that_shares_its_rare_word`)
---

The memory block is the one thing every session pays for before it reads a word of the request.
It was rendered at session start and never again, so a session asking about lanes and a session
asking about the markdown renderer carried byte-identical text: the header, the counts and one
index line per note, capped at 200 lines or 25,000 bytes per store. On this machine 193 recorded
sessions carry that fence, median 1,339 bytes (335 tokens by the `bytes/4` rule the eval uses);
the fixture store of 16 notes, which is what a real store looks like after a few months, costs
3,574 bytes and 885 tokens a session, 163 of its 176 injected note-lines irrelevant to the
letters the case is about.

The first prompt reaches the extension host before the first request, which is the one moment
the block can still be rewritten without touching the frozen prefix (D310), so `MemoryExt` now
answers `PromptSubmitted` as well: it scores every note with the store's own BM25, weights the
types (`feedback` 1.5, `project` 1.4, `user` 1.25, `reference` 1.0), keeps a note whose hook
restates a higher-ranked one as an index row without a body, and spends the start block's row
bytes (never less than two body caps) on the bodies of at most three scored notes before the
remaining index lines. A blank first prompt or a resumed session leaves the block as it stood.
Store failures and load caps stay named, and every cut says what it dropped and how to get it.

The eval is `evals/memory_inject.py` over `evals/fixtures/memory-inject`: eleven real first
messages from `~/.yi/sessions`, hand-labelled against a sixteen-note fixture store with a reason
per label, scored on both arms. The before arm is `before/block.txt`, rendered by the pre-change
build and committed as a fixture; the after arm is rendered by
`crates/runtime/tests/memory_inject.rs` through `memory::focused` and scored by the same script,
so the replay spends nothing and rides `just check`. Run 0088 in docs/eval-ledger.md: recall@body
0.67 against 0.00, precision@body 0.25 against 0.00, 9,462 tokens against 9,735, and 100
irrelevant note-lines against 163 (24 of them carrying a body). The paid judge run (`label --model openrouter/…
--cap-usd 10`) is written but has not run: this session had no network, so the labels are the
owner's agent answering the same four questions the judge prompt asks, and the ledger row says so.
