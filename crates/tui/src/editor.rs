use std::collections::hash_map::RandomState;
use std::fs::{File, OpenOptions};
use std::hash::{BuildHasher, Hasher};
use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use crate::app::App;
use crate::cell::Cell;
use crate::term;

/// Incident: `yi-compose-<pid>.md` in a shared temp directory was world-readable and a link
/// planted at that name was written through; the name now carries a per-call random salt.
pub fn draft_file(dir: &Path) -> std::io::Result<(PathBuf, File)> {
    let salt = RandomState::new().build_hasher().finish();
    let name = format!("yi-compose-{}-{salt:016x}.md", std::process::id());
    let path = dir.join(name);
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    Ok((path, file))
}

/// Raw mode and the keyboard flags come off before the child starts and back on after: the
/// child owns the tty. `restore_tty` is false where there is no terminal to hand over.
pub fn process_pending_editor<B: ratatui::backend::Backend>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    restore_tty: bool,
) {
    if !app.pending_editor {
        return;
    }
    app.pending_editor = false;
    let written = draft_file(&std::env::temp_dir()).and_then(|(path, mut file)| {
        file.write_all(app.composer.text().as_bytes())?;
        Ok(path)
    });
    let Ok(path) = written else {
        app.commit_cell(&Cell::Notice {
            text: "editor: could not write the draft file".to_owned(),
        });
        return;
    };
    // Mode toggles go to stdout, as the startup guard's `Drop` and the panic hook do: raw
    // mode is process-wide and the paste/keyboard escapes ride the same writer.
    let mut out = std::io::stdout();
    if restore_tty {
        term::restore_terminal(&mut out);
    }
    let (editor, status) = yi_runtime::edit_file(&path);
    let reentered = if restore_tty {
        term::enter_terminal(&mut out)
    } else {
        Ok(())
    };
    match status {
        Ok(status) if status.success() => match std::fs::read_to_string(&path) {
            Ok(text) => app.composer.set_text(text.trim_end_matches('\n')),
            Err(error) => app.commit_cell(&Cell::Notice {
                text: format!("editor: could not read the draft back: {error}"),
            }),
        },
        Ok(_) => app.commit_cell(&Cell::Notice {
            text: format!("editor: {editor} exited without saving"),
        }),
        Err(error) => app.commit_cell(&Cell::Notice {
            text: format!("editor: could not run {editor}: {error}"),
        }),
    }
    let _ = std::fs::remove_file(&path);
    if let Err(error) = reentered {
        app.commit_cell(&Cell::Notice {
            text: format!("editor: could not re-enter raw mode: {error}"),
        });
    }
    let _ = terminal.clear();
    app.scheduler.request();
}
