//! Real encoder output, so the request-side image check takes it: Pillow 12.3's 1x1 RGB PNG, and
//! the same image carrying a 7,499,913-byte tEXt chunk, at exactly the 10,000,000-char cap.
pub const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";

pub fn png_at_cap() -> String {
    let head = "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAHJwjXRFWHRrZXkA";
    let tail = "YaWqugAAAAxJREFUeJxjOPt4PwAE8AJwxPF7PgAAAABJRU5ErkJggg==";
    format!("{head}{}{tail}", "QUFB".repeat(2_499_971))
}
