//! The pane table: every pane kind to its definition. A pane is built by filling in its own
//! module; its row is already here.

use yd_core::command::table::Command;
use yd_core::settings::schema::Setting;
use yd_ui::pane::{PaneDef, Scene};

use crate::{
    agents, assets, changes, chat, chat_doc, code_view, containers, daemons, debugger, design,
    files, findings, ipython, logs, lsp, new_chat, notifications, plugin_kit, plugins, remote,
    report, script_pane, settings, terminal, themes, trace, vms, web,
};

pub const PANES: &[&PaneDef] = &[
    &chat::DEF,
    &files::DEF,
    &changes::DEF,
    &code_view::DEF,
    &terminal::DEF,
    &web::DEF,
    &remote::DEF,
    &report::DEF,
    &design::DEF,
    &agents::DEF,
    &trace::DEF,
    &logs::DEF,
    &findings::DEF,
    &assets::DEF,
    &containers::DEF,
    &vms::DEF,
    &daemons::DEF,
    &debugger::DEF,
    &lsp::DEF,
    &plugins::DEF,
    &settings::DEF,
    &themes::DEF,
    &plugin_kit::DEF,
    &script_pane::DEF,
    &ipython::DEF,
];

/// Scenes of the whole-window forms and floating views that are not pane kinds.
pub const FORMS: &[&[Scene]] = &[
    chat_doc::view::SCENES,
    new_chat::view::SCENES,
    notifications::view::SCENES,
];

/// Every scene this crate registers: each pane's, then each form's.
pub fn scenes() -> impl Iterator<Item = &'static Scene> {
    let panes = PANES.iter().flat_map(|def| def.scenes);
    panes.chain(FORMS.iter().copied().flatten())
}

/// Every command a pane adds to the app's own.
pub fn commands() -> impl Iterator<Item = &'static Command> {
    PANES.iter().flat_map(|def| def.commands)
}

/// Every setting a pane adds to the settings page.
pub fn settings() -> impl Iterator<Item = &'static Setting> {
    PANES.iter().flat_map(|def| def.settings)
}
