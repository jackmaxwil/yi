//! The console editor writes a file back as it found it: its mode, its link and its endings.

use std::error::Error;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Command;

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::drive::{open_editor_script, run, session_fixture};
use crate::editor_view::{console, key, palette};
use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[expect(
    clippy::disallowed_methods,
    reason = "printf is the real producer of each shape"
)]
fn printf(path: &Path, format: &str) -> TestResult {
    let out = Command::new("printf").arg(format).output()?;
    if !out.status.success() {
        return Err(format!("printf {format:?} failed").into());
    }
    std::fs::write(path, out.stdout)?;
    Ok(())
}

/// Dies with a saved script that no longer runs: the temp file took the default 0644 and its
/// rename replaced the link with a regular file, leaving the link's target unedited.
#[test]
fn a_saved_script_keeps_its_mode_and_its_link() -> TestResult {
    let dir = Scratch::new("yi-console-save-mode")?;
    let script = dir.join("run.sh");
    printf(&script, "#!/bin/sh\\necho hi\\n")?;
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))?;
    let link = dir.join("link.sh");
    std::os::unix::fs::symlink(&script, &link)?;
    run(
        "editor-save-mode",
        session_fixture(),
        &open_editor_script(
            &link,
            "wait-frame 3000 echo hi\nkey end\ntype  # edited\nwait-frame 3000 .sh ●\n\
             cmd-s\nwait-frame 3000 saved\nquit\n",
        ),
    )?;
    assert!(
        std::fs::symlink_metadata(&link)?.file_type().is_symlink(),
        "the link is still a link"
    );
    assert_eq!(
        std::fs::read_to_string(&script)?,
        "#!/bin/sh # edited\necho hi\n",
        "the edit landed in the link's target"
    );
    let mode = std::fs::metadata(&script)?.permissions().mode() & 0o777;
    assert_eq!(mode, 0o755, "the script keeps its execute bits: {mode:o}");
    Ok(())
}

/// Dies with a file changed by a save that changed nothing: CRLF came back LF, a file without a
/// final newline gained one, and an empty file became one byte.
#[test]
fn an_unchanged_save_writes_the_same_bytes() -> TestResult {
    let dir = Scratch::new("yi-console-save-bytes")?;
    let mut changed = Vec::new();
    for (name, format) in [
        ("crlf.txt", "first\\r\\nsecond\\r\\n"),
        ("open.txt", "no final newline"),
        ("empty.txt", ""),
    ] {
        let path = dir.join(name);
        printf(&path, format)?;
        let before = std::fs::read(&path)?;
        run(
            &format!("editor-save-{name}"),
            session_fixture(),
            &open_editor_script(&path, "cmd-s\nwait-frame 3000 saved\nquit\n"),
        )?;
        let after = std::fs::read(&path)?;
        if after != before {
            changed.push((name, String::from_utf8_lossy(&after).into_owned()));
        }
    }
    assert!(changed.is_empty(), "an unchanged save rewrote: {changed:?}");
    Ok(())
}

/// Opens `path` in a console with no daemon, types `typed` at the cursor and saves.
fn type_and_save(path: &Path, rewrite: Option<&str>, typed: &str) -> TestResult {
    let (mut app, outbound) = console();
    palette(&mut app, &outbound, &format!("e {}", path.display()));
    if let Some(format) = rewrite {
        printf(path, format)?;
        app.tick(&outbound);
    }
    for c in typed.chars() {
        key(
            &mut app,
            &outbound,
            KeyEvent::new(KeyCode::Char(c), KeyModifiers::NONE),
        );
    }
    key(
        &mut app,
        &outbound,
        KeyEvent::new(KeyCode::Char('s'), KeyModifiers::SUPER),
    );
    outbound.shutdown();
    let banner = app.state.banner.unwrap_or_default();
    if banner.starts_with("saved") {
        Ok(())
    } else {
        Err(format!("no save: {banner:?}").into())
    }
}

/// Dies with the agent's file rewritten in the endings read at open: an editor that reloaded the
/// agent's CRLF file saved it back LF and dropped its final newline.
#[test]
fn a_save_after_a_reload_keeps_the_endings_the_agent_wrote() -> TestResult {
    let dir = Scratch::new("yi-console-save-reload")?;
    let path = dir.join("notes.txt");
    printf(&path, "")?;
    type_and_save(&path, Some("hello\\r\\nworld\\r\\n"), "X")?;
    assert_eq!(std::fs::read_to_string(&path)?, "Xhello\r\nworld\r\n");
    Ok(())
}

/// Dies with a new file saved without the newline every other new file ends on, with one CRLF
/// line turning an LF file CRLF, and with a mostly-CRLF file saved back LF.
#[test]
fn a_typed_empty_file_ends_on_a_newline_and_a_mixed_file_keeps_its_majority() -> TestResult {
    let dir = Scratch::new("yi-console-save-typed")?;
    let empty = dir.join("main.rs");
    printf(&empty, "")?;
    type_and_save(&empty, None, "fn main() {}")?;
    assert_eq!(std::fs::read_to_string(&empty)?, "fn main() {}\n");
    let mixed = dir.join("mixed.txt");
    printf(&mixed, "a\\nb\\nc\\r\\nd\\n")?;
    type_and_save(&mixed, None, "")?;
    assert_eq!(std::fs::read_to_string(&mixed)?, "a\nb\nc\nd\n");
    let windows = dir.join("windows.txt");
    printf(&windows, "a\\r\\nb\\r\\nc\\nd\\r\\n")?;
    type_and_save(&windows, None, "")?;
    assert_eq!(std::fs::read_to_string(&windows)?, "a\r\nb\r\nc\r\nd\r\n");
    Ok(())
}
