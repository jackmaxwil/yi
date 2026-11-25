---
issue: Closes #2381
ledger~: TUI | and `session` before the id tile, | and `session` before the id, with no tile beside it,
cut: crates/tui/src/status.rs | crate::colors::name_tile(&input.session_name), | the chip was the first two characters of the session id, and every id starts `01`; the owner chose to remove it rather than give it another meaning
cut: crates/tui/src/colors.rs | pub fn tile_style(name: &str) -> Style { | only the status row's chip called it
---
The status row reads `session 6882dc77`, without the two-cell chip that stood before the id. The chip drew the first two characters of the session id, and a session id is a UUIDv7 whose first characters are its timestamp, so it read `01` for every session.
