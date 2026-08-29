# Rust discipline

Obey the repository's own lints, gates, and conventions first; these rules
cover what lints cannot see.

Make illegal states unrepresentable. A domain value gets a newtype, not a
bare primitive, wherever mixing two of them would compile.

    struct SessionId(String);   fn open(id: SessionId)

Match exhaustively on your own enums. A wildcard arm on a type you own hides
the next variant from the compiler; list the arms instead.

Errors are values. Return `Result<T, E>` with a typed error; reserve
`Option` for genuine absence, not for failure that has a reason.

    fn parse(raw: &str) -> Result<Config, ConfigError>

No panic on a reachable path: no `unwrap`, no `expect`, no indexing that can
be out of range, no `unreachable!` guarding an input. Use `let Some(x) = ..
else`, `get`, and `?`.

    let Some(entry) = table.get(key) else { return Err(Error::Missing) };

Arithmetic on sizes, indexes, and counters is checked or saturating.

    let next = index.saturating_add(1);

Borrow rather than clone; clone rather than fight the borrow checker with
`Rc<RefCell<_>>`. Reach for interior mutability only when shared ownership is
the real requirement.

Prefer iterators to index loops, `&str` to `String` in arguments, and
`impl Trait` in argument position over a generic parameter used once.

Keep a lock guard out of an `await`. Compute under the lock, drop it, then
await.

Public items carry doc comments only where the repository already does.
Otherwise write no comments: names carry the meaning.

Every non-trivial change leaves one runnable check behind, in the crate's own
test style. Run the repository's gate, not just `cargo build`.
