---
issue: Closes #1715
raise: crate console +91, tests +165, comments +8
---
The console's full sidebar already grouped sessions by workspace, but a header said nothing about
how many sessions its group held, and a folded group could only be opened again with a mouse:
the arrow keys walked `visible_rows`, which a fold removes, so the header was not a place the
cursor could stand.

A header now carries its group's count beside its name (`1715 3 ▾`) and PINNED carries its own,
and the cursor steps over the painted order rather than the visible sessions: `Slot::Group` for a
header, `Slot::Row` for a session, so `up` reaches the header above the first row and `enter`
folds or unfolds it. The invariant that pays for the extra state is that the cursor rests on one
slot at a time: `selected` keeps the session the cursor came from, which is where a fold of a
group it sits in still lands. The rail paints no headers, so its slots are its rows and its
navigation is unchanged: switching the sidebar drops a header cursor, and `p` and `c` do nothing
while the cursor rests on a header rather than act on the session `selected` still names.

The counts moved the header text every workspace needle in `tests/drive.rs` matched on, which is
why ten of them now read `demo-root 2 ▾`; the group test also went red for the count's absence,
and that is the change's own red.
