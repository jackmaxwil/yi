---
issue: Closes #1401
raise: crate runtime +62, tests +30, comments +5
---
The runtime's fifteen embedded prompts no longer ride the binary as raw text: `crates/runtime/build.rs` deflates each one and proves the round trip, `yi-runtime::packed::Packed` inflates it once on first use, and every accessor reads through it. The dist binary measured 8,670,176 -> 8,637,152 bytes on this host, **-33,024 bytes**, two 16 KiB `__TEXT` pages back from the 8 MiB cap of D249. `miniz_oxide` was already in the graph through `yi-ai`'s catalog and `yi-orb`, so nothing joins the dependency table and no new crate is linked. Nothing about a prompt's bytes changes: `every_packed_prompt_inflates_to_its_own_file` compares what a session reads with the file it was built from.
