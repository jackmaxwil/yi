use std::time::{Duration, Instant};

use ratatui::crossterm::event::{Event as CtEvent, KeyEventKind};

use crate::app::{App, Bottom, Command};
use crate::approval::AskChoice;
use crate::cell::Cell;
use crate::commands::SLASH_COMMANDS;
use crate::focus::{FocusMove, focus_move, set_focus};
use crate::keymap::{Action, EvalContext, KeyCodeValue, KeyInput, SingleKey};
use crate::plantree::PlanTreeResult;
use crate::popup::{BottomView, ListPopup, PopupResult, walk_files};
use crate::tree::TreeResult;

pub(crate) const ESC_WINDOW: Duration = Duration::from_secs(1);

pub fn handle_terminal_event(
    app: &mut App,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>,
    ct_event: CtEvent,
) {
    app.bind_reply();
    match ct_event {
        CtEvent::Paste(text) => {
            if !app.composer.search_active() {
                app.composer.handle_paste(&text);
            }
            app.scheduler.request();
        }
        CtEvent::Resize(cols, rows) => {
            app.width = usize::from(cols);
            app.set_rows(usize::from(rows));
            app.mark_orb_stale();
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
            if app.plan_tree.is_some() {
                handle_plan_tree_key(app, &key);
                return;
            }
            if app.bottom.is_some() {
                handle_bottom_key(app, &key);
                return;
            }
            let ctx = EvalContext {
                input_empty: app.composer.is_empty(),
            };
            if app.composer.search_active() {
                match app.keymap.resolve(&KeyInput::Single(key), &ctx) {
                    Some(
                        action @ (Action::HistorySearch
                        | Action::Submit
                        | Action::Abort
                        | Action::Quit),
                    ) => handle_action(app, cmd_tx, action),
                    Some(Action::HistoryPrev) => app.composer.search_older(),
                    Some(Action::HistoryNext) => app.composer.search_newer(),
                    Some(_) | None => app.composer.handle_search_key(key_event),
                }
                return;
            }
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

pub(crate) fn handle_plan_tree_key(app: &mut App, key: &SingleKey) {
    let Some(view) = app.plan_tree.as_mut() else {
        return;
    };
    match view.handle_key(key) {
        PlanTreeResult::Open => {}
        PlanTreeResult::Close => app.plan_tree = None,
    }
}

pub(crate) fn handle_bottom_key(app: &mut App, key: &SingleKey) {
    let Some(mut bottom) = app.bottom.take() else {
        return;
    };
    let result = match &mut bottom {
        Bottom::Approval(view, _) => view.handle_key(key),
        Bottom::Command(popup) | Bottom::File(popup) => popup.handle_key(key),
        Bottom::Agents(popup) => popup.handle_key(key),
        Bottom::Model(popup) => popup.handle_key(key),
    };
    match result {
        PopupResult::Open => app.bottom = Some(bottom),
        PopupResult::Close => match bottom {
            Bottom::Approval(view, reply) => {
                let _ = reply.send(view.outcome.unwrap_or(AskChoice::Reject));
            }
            Bottom::Agents(popup) => {
                if let Some(child) = popup.stop {
                    app.stop_child(&child);
                }
            }
            Bottom::Model(popup) => {
                if let Some((model, effort)) = popup.chosen {
                    app.select(model, effort);
                }
            }
            Bottom::Command(_) | Bottom::File(_) => {}
        },
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
            Bottom::Agents(_) | Bottom::Model(_) => {}
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
            if app.composer.search_active() {
                let _ = app.composer.accept_search();
                return;
            }
            if let Some(text) = app.composer.take_submission() {
                // A typed line is a command only when its first word is one (a `/usr/...` path still prompts).
                if let Some(command) = slash_line(&text) {
                    handle_slash(app, &command);
                    return;
                }
                if let Some(bound) = app.reply_bound.take() {
                    let (child_id, question) = (bound.child_id, bound.question);
                    let _ = cmd_tx.send(Command::Answer {
                        child_id,
                        question,
                        text,
                    });
                    return;
                }
                if app.running {
                    app.steering.push(text.clone());
                    let _ = cmd_tx.send(Command::Steer(text));
                } else {
                    app.note_submission();
                    let _ = cmd_tx.send(Command::Prompt(text));
                }
            }
        }
        Action::InsertNewline => app.composer.insert_newline(),
        Action::Abort => handle_escape(app, cmd_tx),
        Action::Quit => {
            if app.composer.cancel_search() {
                return;
            }
            let now = Instant::now();
            if !app.composer.textarea.is_empty() {
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
        Action::HistorySearch => app.composer.begin_search(),
        Action::ToggleExpand => app.cycle_mode(),
        Action::ToggleHud => app.hud_hidden = !app.hud_hidden,
        Action::ExternalEditor => app.pending_editor = true,
        Action::FocusChild => focus_move(app, FocusMove::Child),
        Action::FocusParent => focus_move(app, FocusMove::Parent),
        Action::FocusNextSibling => focus_move(app, FocusMove::Next),
        Action::FocusPrevSibling => focus_move(app, FocusMove::Prev),
        Action::RaiseEffort => app.step_effort(true),
        Action::LowerEffort => app.step_effort(false),
        Action::CycleModel => app.cycle_model(true),
        Action::CycleModelBack => app.cycle_model(false),
        Action::OpenModelPicker => app.open_model_picker(),
    }
}

fn slash_line(text: &str) -> Option<String> {
    let line = text.trim();
    let rest = line.strip_prefix('/')?;
    let head = rest.split_whitespace().next()?;
    SLASH_COMMANDS
        .contains(&head)
        .then(|| rest.trim().to_owned())
}

pub(crate) fn handle_slash(app: &mut App, line: &str) {
    let (command, args) = yi_runtime::slash::split(line);
    match command {
        "new" => app.pending_new = true,
        "undo" => app.pending_undo = true,
        "quit" => app.quit = true,
        "tree" => app.pending_open_tree = true,
        "plantree" => app.pending_open_plan_tree = true,
        "editor" => app.pending_editor = true,
        "agents" => app.open_agents(),
        "model" => app.open_model_picker(),
        "advisor" | "plan" | "goal" | "permissions" | "compact" | "sessions" | "heartbeat" => {
            app.pending_command = Some(if args.is_empty() {
                command.to_owned()
            } else {
                format!("{command} {args}")
            });
        }
        "lanes" | "land" | "discard" | "pr" | "base" => {
            app.pending_slash = Some(line.trim().to_owned());
        }
        _ => app.commit_cell(&Cell::Notice {
            text: format!("unknown command: /{command}"),
        }),
    }
    app.scheduler.request();
}

pub(crate) fn handle_escape(app: &mut App, cmd_tx: &tokio::sync::mpsc::UnboundedSender<Command>) {
    if app.composer.cancel_search() {
        return;
    }
    let now = Instant::now();
    if app.focused.is_some() {
        set_focus(app, None);
        return;
    }
    if app.running {
        match app.esc_armed_at {
            Some(at) if now.duration_since(at) < ESC_WINDOW => {
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

impl App {
    /// An open approval never takes it (closing it rejects the tool call); a closed box takes it
    /// when it holds any text, the one predicate `Action::Quit` clears on.
    pub fn takes_ctrl_c(&self) -> bool {
        match &self.bottom {
            Some(Bottom::Approval(..)) => false,
            Some(_) => true,
            None => !self.composer.textarea.is_empty(),
        }
    }
}
