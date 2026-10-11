---
issue: Closes #1211
raise: crate cli +3, crate runtime +94, comments +7, tests +14
---
The line selector now answers a large cut by asking in parallel parts (Closes #1211). Its hidden lines go out as concurrent requests of at most 300 lines (`PART_LINES`), all under the one 5 s deadline, each held to an even share of the 200-line budget so a part that names everything cannot spend the others' lines; the ranges merge, each part's cost is booked, and a part that gave no answer is named in the row with the parts asked. A cold single request over a 3,000-line log missed the deadline three times in five; in parts, ten cold tries all answered and nine found the failing line (eval-ledger 0083, 0084).
