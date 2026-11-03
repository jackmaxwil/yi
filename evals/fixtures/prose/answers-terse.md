## Answer 1

The assertion runs before the fixture is written.

- `beforeEach` runs after this `it` block.
- The row the test reads is inserted by a sibling test.

Move the insert into the test, or move the assertion into a `beforeEach` of its own. Then rerun:

```
cargo test -p parser -- fails_before_setup
```

## Answer 2

Each turn is a request built from the session's entries, then a dispatch of the tool calls the stream asks for.

- The provider streams events back.
- Every tool call is dispatched before the next request.
- The loop ends when the model asks for no more tools.

`run_loop` is infallible: a failure is an event, not a return value.

## Answer 3

Read the tests first. They pin what the parser must keep doing.

1. Name the seam the file already has.
2. Move one function per commit.
3. Rerun the parser tests after each move, and keep them unchanged.

The refactor is mechanical once the seam is named. The seam is the work.
