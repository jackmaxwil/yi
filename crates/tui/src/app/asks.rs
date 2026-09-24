use serde_json::Value;
use yi_types::subagent::ChildFlag;

use super::App;
use crate::cell::TaskStatus;

impl App {
    pub(crate) fn reply_target(&self) -> Option<(String, String)> {
        let id = self.focused.as_ref()?;
        let state = self.tasks.get(id)?;
        match (state.cell.status, &state.cell.flag) {
            (TaskStatus::Running, Some(ChildFlag::NeedsYou { note }))
                if note.starts_with("asks ") =>
            {
                let title = format!(" reply to {}: {note} ", state.cell.description);
                Some((id.clone(), title))
            }
            _ => None,
        }
    }
}

pub(crate) fn mail_source(custom_type: &str, details: Option<&Value>) -> String {
    let field = |key: &str| details.and_then(|details| details.get(key)?.as_str());
    match (custom_type, field("from"), field("to")) {
        ("agent_message", Some(from), Some(to)) => {
            let kind = field("kind")
                .filter(|kind| *kind != "inform")
                .unwrap_or("mail");
            format!("{kind} {from} → {to}")
        }
        _ => custom_type.to_owned(),
    }
}
