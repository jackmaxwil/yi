use std::path::{Path, PathBuf};

use super::{Effect, Event, EventMask, Extension, Rank, Slot};

const FRAGMENT: &str = include_str!("../prompts/grid.md");

pub struct Grid {
    cwd: PathBuf,
    home: PathBuf,
}

impl Grid {
    pub fn new(cwd: PathBuf, home: PathBuf) -> Self {
        Self { cwd, home }
    }

    fn binary_present(&self) -> bool {
        let mut candidates = vec![
            self.home.join(".cargo/bin/grid"),
            PathBuf::from("/usr/local/bin/grid"),
            PathBuf::from("/opt/homebrew/bin/grid"),
        ];
        if let Some(path) = std::env::var_os("PATH") {
            candidates.extend(std::env::split_paths(&path).map(|dir| dir.join("grid")));
        }
        candidates.iter().any(|candidate| candidate.is_file())
    }

    fn chartable(&self) -> bool {
        ["Cargo.toml", "pyproject.toml", "setup.py"]
            .iter()
            .any(|marker| self.cwd.join(marker).exists())
            && dir_has(&self.cwd, ".git")
    }
}

fn dir_has(dir: &Path, name: &str) -> bool {
    dir.join(name).exists()
}

impl Extension for Grid {
    fn name(&self) -> &'static str {
        "grid"
    }

    fn interests(&self) -> EventMask {
        EventMask::SESSION_START
    }

    fn on(&mut self, event: &Event, out: &mut Vec<Effect>) {
        if !matches!(event, Event::SessionStart { .. }) {
            return;
        }
        if self.chartable() && self.binary_present() {
            out.push(Effect::AttachFragment {
                slot: Slot::new(Rank::Tool, "grid"),
                text: FRAGMENT.to_owned(),
            });
        }
    }
}
