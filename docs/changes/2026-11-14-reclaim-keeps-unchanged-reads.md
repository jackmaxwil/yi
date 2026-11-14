---
issue: Closes #2211
growth: +167 the unchanged-file check (per-path hash memo keyed by mtime and length), the context-share gate, and the cwd the overlay needs to resolve a header's path; weighed for deletion: Sections already parsed the headers, so the parse is reused and only the tag is kept beside the path
raise: crate runtime +167, tests +162, comments +2
decision: reclaim keeps a read or grep result past the age floor while every `[path#TAG]` it printed matches the file on disk and the last reply used under `reclaim.keep_below_pct` (30) of the window (amends D597) | ten Sonnet 5.5 children reread a path they had already read 518 times and repeated 197 exact calls because results aged out at five requests while under 10% of the window was used; 396 of the 404 rereads past the floor were of a file whose tag had not changed | reversible via `reclaim.keep_below_pct = 0`, which restores the age-floor-only rule
cut: none crates/runtime/src/reclaim.rs
---
A file read or search result is no longer cut after five requests when the file has not changed and the conversation is still small. Every file the result names must still match the tag the tool printed, and the last reply must have used under 30% of the model's window (`reclaim.keep_below_pct`, 0 turns it off). A file that changed, a deleted file, or a fuller window ages the result out as before. The cut notice now names this exception. Output from `bash` commands such as `sed -n` or `cat` carries no tag and still ages out at five requests.
