use std::sync::Arc;

use yi_runtime::plan::ops::OpSink;

/// The session opened `id`: one op in its ledger, the way the engine's sink records it. A
/// plan file no session's ledger names is nobody's plan.
pub fn own(store: &yi_session::SharedSession, id: &str) -> Result<(), Box<dyn std::error::Error>> {
    let handle = store.clone();
    yi_runtime::plan::ledger::SessionOpSink(Arc::new(move || Some(handle.clone()))).record(
        yi_types::plan::ledger::PlanOpRecord {
            plan: yi_types::plan::doc::PlanId::new(id)?,
            op: "open".to_owned(),
            actor: "owner".to_owned(),
            at: 0,
            todo: None,
            from: None,
            to: None,
            todos: 1,
            extra: serde_json::Map::new(),
        },
    )?;
    Ok(())
}
