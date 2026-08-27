use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event as CtEvent, KeyEventKind};

use crate::app::{App, Bottom, Command, SLASH_COMMANDS};
use crate::approval::AskChoice;
use crate::cell::{Cell, TranscriptMode};
use crate::focus::{FocusMove, focus_move, set_focus};
use crate::keymap::{Action, EvalContext, KeyCodeValue, KeyInput, SingleKey};
use crate::popup::{BottomView, ListPopup, PopupResult, walk_files};
use crate::tree::TreeResult;

pub(crate) fn handle_terminal_event(
    app: &mut App,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ct_event: CtEvent,
) {
    match ct_event {
        CtEvent::Paste(text) => {
            app.composer.handle_paste(&text);
            app.scheduler.request();
        }
        CtEvent::Resize(cols, rows) => {
            app.width = usize::from(cols);
            app.set_rows(usize::from(rows));
            app.scheduler.request();
        }
        CtEvent::Key(key_event) if key_event.kind != KeyEventKind::Release => {
            let Some(key) = SingleKey::from_event(&key_event) else {
                return;
            };
            app.scheduler.request();
            if app.tree.is_some() {
                handle_tree_key(app, &key);
                return;
            }
            if app.bottom.is_some() {
                handle_bottom_key(app, &key);
                return;
            }
            let ctx = EvalContext {
                input_empty: app.composer.is_empty(),
            };
            match app.keymap.resolve(&KeyInput::Single(key), &ctx) {
                Some(action) => handle_action(app, cmd_tx, action),
                None => {
                    if key.code == KeyCodeValue::Char('/')
                        && !key.ctrl
                        && !key.alt
                        && app.composer.is_empty()
                    {
                        app.bottom = Some(Bottom::Command(ListPopup::new(
                            '/',
                            SLASH_COMMANDS.iter().map(|s| (*s).to_owned()).collect(),
                        )));
                    } else if key.code == KeyCodeValue::Char('@') && !key.ctrl && !key.alt {
                        let files = walk_files(std::path::Path::new(&app.options.cwd), 100);
                        app.bottom = Some(Bottom::File(ListPopup::new('@', files)));
                    } else if key.code == KeyCodeValue::Backspace && !key.ctrl && !key.alt {
                        app.composer.backspace();
                    } else {
                        app.composer.input(key_event);
                    }
                }
            }
        }
        _ => {}
    }
}

pub(crate) fn handle_tree_key(app: &mut App, key: &SingleKey) {
    let Some(tree) = app.tree.as_mut() else {
        return;
    };
    match tree.handle_key(key) {
        TreeResult::Open => {}
        TreeResult::Close => app.tree = None,
        TreeResult::Rewind(id) => {
            app.tree = None;
            app.pending_rewind = Some(id);
        }
    }
}

pub(crate) fn handle_bottom_key(app: &mut App, key: &SingleKey) {
    let Some(mut bottom) = app.bottom.take() else {
        return;
    };
    let result = match &mut bottom {
        Bottom::Approval(view, _) => view.handle_key(key),
        Bottom::Command(popup) | Bottom::File(popup) => popup.handle_key(key),
    };
    match result {
        PopupResult::Open => app.bottom = Some(bottom),
        PopupResult::Close => {
            if let Bottom::Approval(view, reply) = bottom {
                let _ = reply.send(view.outcome.unwrap_or(AskChoice::Reject));
            }
        }
        PopupResult::Insert(text) => match bottom {
            Bottom::Command(_) => {
                let command = text.trim_start_matches('/').to_owned();
                handle_slash(app, &command);
            }
            Bottom::File(_) => {
                app.composer
                    .textarea
                    .insert_str(format!("{} ", text.trim_start_matches('@')));
            }
            Bottom::Approval(view, reply) => {
                let _ = reply.send(view.outcome.unwrap_or(AskChoice::Reject));
            }
        },
    }
}

pub(crate) fn handle_action(
    app: &mut App,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    action: Action,
) {
    match action {
        Action::Submit => {
            if let Some(text) = app.composer.take_submission() {
                if app.running {
                    app.steering.push(text.clone());
                    let _ = cmd_tx.send(Command::Steer(text));
                } else {
                    let _ = cmd_tx.send(Command::Prompt(text));
                }
            }
        }
        Action::InsertNewline => app.composer.insert_newline(),
        Action::Abort => handle_escape(app, cmd_tx),
        Action::Quit => {
            let now = Instant::now();
            if !app.composer.is_empty() {
                app.composer.set_text("");
                return;
            }
            match app.ctrl_c_at {
                Some(at) if now.duration_since(at) < Duration::from_secs(1) => app.quit = true,
                _ => {
                    if app.running {
                        let _ = cmd_tx.send(Command::Abort);
                    }
                    app.ctrl_c_at = Some(now);
                }
            }
        }
        Action::HistoryPrev => app.composer.history_prev(),
        Action::HistoryNext => app.composer.history_next(),
        Action::ToggleExpand => {
            app.mode = app.mode.next();
            app.commit_cell(&Cell::Notice {
                text: format!("transcript mode: {}", app.mode.label()),
            });
        }
        Action::ToggleHud => app.hud_hidden = !app.hud_hidden,
        Action::ExternalEditor => app.pending_editor = true,
        Action::FocusChild => focus_move(app, FocusMove::Child),
        Action::FocusParent => focus_move(app, FocusMove::Parent),
        Action::FocusNextSibling => focus_move(app, FocusMove::Next),
        Action::FocusPrevSibling => focus_move(app, FocusMove::Prev),
    }
}

pub(crate) fn handle_slash(app: &mut App, command: &str) {
    match command {
        "new" => app.pending_new = true,
        "quit" => app.quit = true,
        "tree" => app.pending_open_tree = true,
        "editor" => app.pending_editor = true,
        "expand" => {
            if let Some(cell) = app.last_finished_tool.clone() {
                let width = app.width.saturating_sub(2);
                let lines = Cell::Tool(cell).lines(width, &app.theme, TranscriptMode::Verbose, 0);
                app.pending_commit.extend(lines);
            }
        }
        _ => app.commit_cell(&Cell::Notice {
            text: format!("unknown command: /{command}"),
        }),
    }
    app.scheduler.request();
}

pub(crate) fn handle_escape(app: &mut App, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
    let now = Instant::now();
    if app.focused.is_some() {
        set_focus(app, None);
        return;
    }
    if app.running {
        match app.esc_armed_at {
            Some(at) if now.duration_since(at) < Duration::from_secs(1) => {
                let _ = cmd_tx.send(Command::Abort);
                app.esc_armed_at = None;
            }
            _ => app.esc_armed_at = Some(now),
        }
        return;
    }
    if !app.composer.is_empty() {
        return;
    }
    match app.last_esc_at {
        Some(at) if now.duration_since(at) < Duration::from_millis(500) => {
            app.last_esc_at = None;
            app.pending_open_tree = true;
        }
        _ => app.last_esc_at = Some(now),
    }
}
