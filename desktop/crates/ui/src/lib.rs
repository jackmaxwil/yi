//! The design system and the engines on Rune: tokens, shared components, pane chrome, the
//! grid view, the row list, the code, terminal and screen surfaces, the overlay and menus.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod a11y;
pub mod chrome;
pub mod code;
pub mod disk;
pub mod grid_view;
pub mod kit;
pub mod list;
pub mod menu;
pub mod overlay;
pub mod pane;
pub mod scenes;
pub mod screen;
pub mod shown;
pub mod slots;
pub mod states;
pub mod term;
pub mod theme;
pub mod win;
