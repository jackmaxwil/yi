---
issue: Closes #2110
cut: none crates/permission/src/catastrophic.rs
cut: none crates/tools/src/hashline/tool.rs
cut: none crates/tools/src/hashline/normalize.rs
raise: crate permission +22, crate session +6, tests +186, comments +6
---
Grep, read, the read gate and fork copy less, and every result they return is byte for byte what it was. `split` takes the file it splits by value and hands a file with no CR and no BOM straight back, with no copy and no per-line terminator list, since restoring it writes `\n` everywhere, so grep copies none of the plain files it searches. `normalize_to_lf` and the binary check use the std scans. Read walks one forward iterator across its sorted windows instead of holding a `Vec` of every line, so a 16 MiB file of newlines no longer costs 600 MB, and two hundred ranges near the end of a large file cost one pass. The read gate normalizes its protected trees once per gate instead of once per link hop, and its walls and spared paths once per check instead of once per hop, and it does not canonicalize a read's path a second time; the identities of the workspace git dirs still come from the spelling the kernel resolves, so a cwd spelled `link/../proj` keeps its `.git` walled (D323). Fork builds its store from the mutations it just wrote instead of reading and parsing the file again.
