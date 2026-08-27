use crate::app::App;
use crate::cell::Cell;
use crate::term;

/// Raw mode and the keyboard flags come off before the child starts and go back
/// on after it exits: the child owns the tty in between. `restore_tty` is false
/// where there is no terminal to hand over (headless drive mode, tests).
pub fn process_pending_editor<B: ratatui::backend::Backend>(
    app: &mut App,
    terminal: &mut crate::terminal::Terminal<B>,
    restore_tty: bool,
) {
    if !app.pending_editor {
        return;
    }
    app.pending_editor = false;
    let path = std::env::temp_dir().join(format!("yi-compose-{}.md", std::process::id()));
    if std::fs::write(&path, app.composer.text()).is_err() {
        app.commit_cell(&Cell::Notice {
            text: "editor: could not write the draft file".to_owned(),
        });
        return;
    }
    // Mode toggles go to stdout, as the startup guard's `Drop` and the panic
    // hook already do: raw mode is process-wide, and the paste/keyboard escapes
    // ride the same writer those two use.
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
