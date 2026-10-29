---
issue: Closes #1589
raise: tests +92, crate runtime +12, crate types +3
---
A checklist `set` from the tool no longer writes every row it did not name into its own journal record. The op carries `keep_rest`, which the reducer honours by keeping the unnamed rows, and `keep_omitted` sets it instead of padding the rows; a 30-todo plan's two-row set went from a 1,769-byte record to one holding two rows. A set that has to open a plan still carries the rows there, because a new plan is creating them rather than keeping them. Old records replay unchanged: the flag defaults to false and the padded rows they already hold stay the whole cut.
