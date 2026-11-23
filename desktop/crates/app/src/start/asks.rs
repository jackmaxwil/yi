//! The approvals that wait, kept in a file of their own so a killed app can show them again.

use std::io::Write;
use std::path::{Path, PathBuf};

use yd_core::findings::triage;
use yd_core::msg::Msg;
use yd_core::wire::event::WireEvent;
use yd_core::wire::frame::Frame;
use yd_ui::win::Hub;

use super::Kept;

/// Puts back the asks a killed app kept, as if the server had sent them again. A line that
/// is not an ask is skipped: the file is the user's to edit.
pub(super) fn restore(hub: &mut Hub, path: &Path) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut text = String::new();
    if std::io::Read::read_to_string(&mut std::io::Read::take(file, triage::MAX_FILE), &mut text)
        .is_err()
    {
        return;
    }
    for line in text.lines().take(yd_core::reduce::approvals::MAX_ASKS) {
        if let Ok(Frame::Ask { .. }) = Frame::decode(line.as_bytes()) {
            let line = WireEvent::Line(line.to_owned());
            yd_core::msg::step(&mut hub.model, Msg::Wire(line));
        }
    }
}

/// The asks to write and where, when they changed since they were last handed over.
pub(super) fn unsaved(hub: &mut Hub) -> Option<(PathBuf, String)> {
    let asks = hub.model.wire.kept();
    let kept = hub.panes.get_mut::<Kept>(None)?;
    let path = kept.asks.clone().filter(|_| kept.written != asks)?;
    kept.written.clone_from(&asks);
    Some((path, asks))
}

pub(super) fn save(path: &Path, asks: &str) {
    if let Err(why) = triage::replace(path, &path.with_extension("jsonl.tmp"), asks.as_bytes()) {
        let path = path.display();
        let _ = writeln!(std::io::stderr(), "yi-desktop: {path}: {why}");
    }
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use yd_ui::disk::Disk;

    use super::super::{Files, files, keep};
    use super::Path;

    #[test]
    fn an_approval_waiting_when_the_app_was_killed_waits_again_after_a_restart()
    -> Result<(), Box<dyn Error>> {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/df10-asks");
        let folder = dir.join(std::process::id().to_string());
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder)?;
        let mut killed = crate::root::init();
        {
            let mut hub = killed.hub.try_borrow_mut()?;
            files(&mut hub, Files::under(&folder));
            let whole = crate::shell::view::SCENES.iter();
            let scene = whole.clone().find(|scene| scene.name == "shell/everything");
            (scene.ok_or("no scene")?.setup)(&mut hub);
        }
        keep(&mut killed);
        let asked: Vec<String> = {
            let mut hub = killed.hub.try_borrow_mut()?;
            hub.panes.get_mut::<Disk>(None).ok_or("no disk")?.finish();
            hub.model
                .wire
                .waiting()
                .map(|ask| ask.id.as_str().to_owned())
                .collect()
        };
        assert!(!asked.is_empty(), "an approval waits");
        let again = crate::root::init();
        let mut hub = again.hub.try_borrow_mut()?;
        files(&mut hub, Files::under(&folder));
        let shown: Vec<String> = hub
            .model
            .wire
            .waiting()
            .map(|a| a.id.as_str().to_owned())
            .collect();
        assert_eq!(
            shown, asked,
            "the restart forgot the approval Yi still waits on"
        );
        drop(hub);
        std::fs::remove_dir_all(&folder)?;
        Ok(())
    }
}
