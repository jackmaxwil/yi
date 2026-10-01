---
issue: Closes #1008
---
Review rounds report high and medium findings only, and weigh what a PR is for over test coverage
(Closes #1008). Over 194 rounds, half of 1,520 findings were low and none was fixed, and the tests
lens made 43 of the 74 highs. Every lens now reads the PR's "Why needed" and reports at most five
findings, each naming its consequence. Three lenses replace necessity and simplify: intent (does the
diff do what the PR is for, and nothing past it), reuse (a twin the repository already has, cited
by address) and bloat (needless layers, public surface and sprawl). The tests lens judges only
whether the proof the PR offers is real, and stops asking for more unit tests.
