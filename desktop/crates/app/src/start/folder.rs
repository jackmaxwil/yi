use std::path::Path;

use yd_core::ids::PaneId;
use yd_sources::pty::Screen;
use yd_ui::win::Hub;

/// The kept folder if it is a directory, else the project's, else home, with the line saying
/// why it changed; never a folder that is not there.
pub(super) fn starting_folder(
    kept: String,
    project: Option<String>,
    home: Option<String>,
) -> (String, Option<String>) {
    let usable = |folder: &String| !folder.is_empty() && Path::new(folder).is_dir();
    if usable(&kept) {
        return (kept, None);
    }
    let why = match kept.is_empty() {
        true => "No folder is known for this shell".to_owned(),
        false => format!("The folder {kept} is gone"),
    };
    let project = project
        .filter(usable)
        .map(|folder| (folder, "the project folder"));
    let home = home
        .filter(usable)
        .map(|folder| (folder, "your home folder"));
    match project.or(home) {
        Some((folder, place)) => {
            let said = format!("{why}: started in {place}, {folder}");
            (folder, Some(said))
        }
        None => (kept, None),
    }
}

/// Where a shell of `pane` that names no folder starts: the one New chat points at, else the
/// folder its thread works in, else the one New chat would start in, which is home last.
pub(super) fn of_pane(hub: &Hub, pane: PaneId) -> Option<String> {
    let window = hub.model.grid.window_of(pane).unwrap_or(crate::root::MAIN);
    let chosen = yd_ui::shown::chosen(hub, window).map(str::to_owned);
    let works =
        || yd_ui::shown::of_pane(hub, pane).map(|t| yd_ui::shown::works_in(hub, t).to_owned());
    chosen
        .or_else(works)
        .or_else(|| yd_panes::new_chat::view::folder(hub, window))
}

/// The folder a shell on `screen` starts in, the muted line saying why it moved written to it.
pub(super) fn start_in(screen: &Screen, kept: String, project: Option<String>) -> String {
    let home = std::env::var("HOME").ok();
    let (folder, said) = starting_folder(kept, project, home);
    if let Some(said) = said {
        screen.take(format!("\x1b[90m{said}\x1b[0m\r\n").as_bytes());
    }
    folder
}

#[cfg(test)]
mod tests {
    use std::error::Error;
    use std::rc::Rc;

    use yd_core::ids::Millis;

    use super::super::{Clock, Running, inbox};
    use super::*;

    /// Incident: a restored Terminal said "the folder  is gone" because the shown thread had not
    /// been listed yet, its folder empty; the shell must start in a folder that exists.
    #[test]
    fn a_shell_of_a_thread_with_no_known_folder_starts_in_a_real_one_and_says_so()
    -> Result<(), Box<dyn Error>> {
        use yd_core::ids::{ThreadId, WindowId};
        use yd_core::reduce::threads::Thread;
        use yd_ui::pane::SCENE_PANE;
        use yd_ui::term::view::state::Shells;

        let win = crate::root::init();
        let feed = {
            let mut hub = win.hub.try_borrow_mut().map_err(|_| "hub")?;
            let id = ThreadId::new("restored");
            hub.model
                .wire
                .threads
                .insert(id.clone(), Thread::new(id.clone()));
            yd_ui::shown::pick(&mut hub, WindowId(0), id);
            let shells = hub
                .panes
                .get_mut::<Shells>(Some(SCENE_PANE))
                .ok_or("no shells")?;
            shells.tabs.first().ok_or("no tab")?.feed
        };
        let (sender, _inbox) = inbox(|| {});
        let clock: Clock = std::sync::Arc::new(|| Millis(0));
        let running = Running::new(Rc::downgrade(&win.hub), sender, clock);
        let (screen, spawn) = running.shell(feed).ok_or("no shell")?;
        assert!(spawn.cwd.is_dir(), "started in {:?}", spawn.cwd);
        let said = screen
            .parser
            .lock()
            .map_err(|_| "parser")?
            .screen()
            .contents();
        assert!(said.contains("started in"), "the pane says where: {said:?}");
        Ok(())
    }

    /// A kept folder that was released falls to the project's, then home; a real one stays.
    #[test]
    fn a_kept_folder_that_is_gone_falls_to_the_project_then_home() {
        let here = env!("CARGO_MANIFEST_DIR").to_owned();
        let gone = "/nowhere/lane/3".to_owned();
        let (folder, said) = starting_folder(here.clone(), None, None);
        assert_eq!((folder, said), (here.clone(), None));
        let (folder, said) = starting_folder(gone.clone(), Some(here.clone()), None);
        assert_eq!(folder, here);
        assert!(said.is_some_and(|said| said.contains("/nowhere/lane/3 is gone")));
        let (folder, said) = starting_folder(gone.clone(), Some(gone.clone()), Some(here.clone()));
        assert_eq!(folder, here);
        assert!(said.is_some_and(|said| said.contains("your home folder")));
        assert_eq!(starting_folder(gone.clone(), None, None), (gone, None));
    }
}
