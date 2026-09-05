#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

pub mod agents;
pub mod app;
pub mod approval;
pub mod capture;
pub mod cell;
pub mod colors;
pub mod commands;
pub mod composer;
pub mod diffview;
pub mod drive;
pub mod editor;
pub mod focus;
pub mod frame;
pub mod highlight;
pub mod history;
pub mod hud;
pub mod input;
pub mod keymap;
pub mod logo;
pub mod markdown;
pub mod model;
pub mod motion;
pub mod orb;
pub mod plantree;
pub mod popup;
pub mod port;
pub mod pycell;
pub mod reflow;
pub mod render;
pub mod reveal;
pub mod status;
pub mod table;
pub mod term;
pub mod terminal;
pub mod transcript;
pub mod tree;
pub mod wrap;

pub use app::{AskRequest, Command, TuiOptions, UiEvent, run_tui};
pub use approval::AskChoice;
pub use drive::{DriveOptions, parse_script, run_headless};
pub use port::{Answer, Reply, SessionPort, tick};
