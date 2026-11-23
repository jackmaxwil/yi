//! One app a home folder: a second launch asks the running one forward and exits.

use std::fs::{File, OpenOptions, TryLockError};
use std::io;
use std::path::{Path, PathBuf};

use yd_ui::win::{Hub, Win};

const LOCK: &str = "running.lock";
const ASK: &str = "show.ask";

/// The running app's hold on its folder, kept in the hub (under no pane) for the whole run:
/// the lock goes when the process does, however it ends.
#[derive(Debug, Default)]
pub struct Single {
    held: Option<(File, PathBuf)>,
}

/// Takes `folder` for this run. None when another run holds it: that run is asked forward.
pub fn hold(folder: &Path) -> io::Result<Option<Single>> {
    std::fs::create_dir_all(folder)?;
    let mut open = OpenOptions::new();
    let file = open.create(true).truncate(false).write(true);
    let file = file.open(folder.join(LOCK))?;
    match file.try_lock() {
        Ok(()) => Ok(Some(Single {
            held: Some((file, folder.to_path_buf())),
        })),
        Err(TryLockError::WouldBlock) => {
            std::fs::write(folder.join(ASK), b"")?;
            Ok(None)
        }
        Err(TryLockError::Error(error)) => Err(error),
    }
}

/// Whether a second launch asked this run forward since the last look; the ask is spent.
pub fn asked(hub: &Hub) -> bool {
    let held = hub.panes.get::<Single>(None).and_then(|s| s.held.as_ref());
    held.is_some_and(|(_, folder)| std::fs::remove_file(folder.join(ASK)).is_ok())
}

/// Brings every window forward when a second launch asked; the main window's poll calls it.
pub fn answer(win: &Win) -> bool {
    let Ok(hub) = win.hub.try_borrow() else {
        return false;
    };
    let asked = asked(&hub);
    if asked {
        crate::menubar::raise(&hub);
    }
    asked
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use super::*;

    #[test]
    fn a_second_launch_in_the_same_home_asks_the_first_forward_and_another_home_runs()
    -> Result<(), Box<dyn Error>> {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/df13-single")
            .join(std::process::id().to_string());
        let (home, other) = (root.join("home"), root.join("other"));
        let first = hold(&home)?.ok_or("the first run did not hold its home")?;
        assert!(
            hold(&home)?.is_none(),
            "a second copy in the same home runs"
        );
        assert!(
            hold(&other)?.is_some(),
            "an instance with its own home is refused"
        );
        let hub = Hub::new(&[], Vec::new());
        let mut hub = hub.try_borrow_mut()?;
        if let Some(kept) = hub.panes.get_mut::<Single>(None) {
            *kept = first;
        }
        assert!(asked(&hub), "the first never hears the ask");
        assert!(!asked(&hub), "one ask brings it forward once");
        Ok(())
    }
}
