## Answer 1

That failure is crucial to understand, because the assertion runs before the fixture has been written. The fixture is created in the `beforeEach` hook, which serves as the setup step for every case, ensuring that the table is in a known state. When the test reads the row it is not just waiting on a promise but reading a table that no hook has written yet — a seamless race that experts say is the most common cause of this pattern in practice.

In terms of what to do next, the recommended approach is to move the fixture into the test itself, and to make the setup explicit there. This highlights the ordering the framework already guarantees, and it is a robust improvement to the suite.

Overall, the failure is a common one and the fix is straightforward.

## Answer 2

The agent loop is pivotal to how the runtime works, and it plays a role in every turn the session takes. Each turn builds a request from the session's entries, which serves as the input to the provider, and then it reads the events back off the stream, ensuring that every tool call the model asks for is dispatched before the next request is built. This is not just a loop but a state machine, and it is an intricate boundary between the provider and the tools.

The loop terminates when the model stops asking for tools, which experts note is the usual case for a short answer. It is a robust design that has held up well across providers.

In short, it is a request-and-dispatch loop with a termination condition.

## Answer 3

Planning a refactor of the parser is a multifaceted task, and the first thing to do is to read the parser's tests. The tests serve as the ground truth for what the parser must keep doing, ensuring that the refactor does not change behaviour, and they highlight the shapes the caller depends on. It is not just a matter of moving code around but of finding the seam the file already has, and the seam is the interesting question here.

A plan of this kind benefits from small steps, and each step should end with the suite green. Widely regarded practice is to move one function at a time, and to keep the tests unchanged throughout, which is a dependable way to work.

In conclusion, the refactor is mostly mechanical once the seam is named.
