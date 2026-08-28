use std::sync::Arc;

use yi_runtime::AgentSession;

use crate::app::App;
use crate::cell::Cell;

/// V11 is a user act: no host request exists, so only this command promotes.
pub fn process_pending_advisor(app: &mut App, session: &Arc<AgentSession>) {
    let Some(args) = app.pending_advisor.take() else {
        return;
    };
    let Some(advisor) = session.advisor() else {
        app.commit_cell(&Cell::Notice {
            text: "/advisor: no advisor is attached to this session".to_owned(),
        });
        app.scheduler.request();
        return;
    };
    let text = match args.split_once(char::is_whitespace) {
        Some(("promote", id)) => match yi_runtime::advisor::promote_advice(session, id.trim()) {
            Ok(path) => format!(
                "/advisor promote {}: armed now and standing in {}",
                id.trim(),
                path.display()
            ),
            Err(error) => format!("/advisor promote: {error}"),
        },
        _ if args.trim() == "promote" => {
            "/advisor promote <advice-id> — the id is in the advisory's details".to_owned()
        }
        _ => {
            let promotable = advisor.promotable();
            let listed = if promotable.is_empty() {
                "nothing to promote yet".to_owned()
            } else {
                promotable
                    .iter()
                    .rev()
                    .take(5)
                    .map(|(id, advice)| format!("  {id}  {advice}"))
                    .collect::<Vec<_>>()
                    .join("\n")
            };
            format!("{}\npromotable:\n{listed}", advisor.stats())
        }
    };
    app.commit_cell(&Cell::Notice { text });
    app.scheduler.request();
}
