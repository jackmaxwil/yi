//! The rules' gate: each recorded session, replayed through the reducer into a window that
//! starts with the chat alone, ends with the panes the rules say of it.

use std::collections::BTreeSet;

use yd_core::pane::PaneKind::{self, Agents, Chat, IPython};

use super::ipython::window;
use super::*;

macro_rules! recorded {
    ($name:literal) => {
        include_str!(concat!("../../../../../../fixtures/wire/", $name, ".jsonl"))
    };
}

/// Each recording with the panes the rules say it ends with. Changes and the Web app are told
/// by this Mac, not by a session, so no recording brings them.
const SAID: [(&str, &str, &[PaneKind]); 6] = [
    ("turn", recorded!("turn"), &[Chat]),
    ("approval-allow", recorded!("approval-allow"), &[Chat]),
    ("tools-turn", recorded!("tools-turn"), &[Chat]),
    ("child-turn", recorded!("child-turn"), &[Chat, Agents]),
    ("child-ask", recorded!("child-ask"), &[Chat, Agents]),
    ("kernel-cells", recorded!("kernel-cells"), &[Chat, IPython]),
];

/// The table is read off the recordings themselves, not off what the app made of them: a
/// child agent's update on the wire is Agents' fact, a picture block is IPython's.
#[test]
fn a_recorded_session_ends_with_the_panes_the_rules_say() -> Check {
    for (name, recording, said) in SAID {
        let child = recording.contains("\"_yi/subagent_update\"");
        let picture = recording.contains("\"type\":\"image\"");
        assert_eq!(said.contains(&Agents), child, "{name}: a child agent");
        assert_eq!(said.contains(&IPython), picture, "{name}: a picture");
        let runtime = window(recording, usize::MAX)?;
        let hub = runtime.state.hub.try_borrow()?;
        let grid = &hub.model.grid;
        let layout = &grid.window(root::MAIN).ok_or("no window")?.layout;
        let held = grid.panes.iter().filter(|pane| layout.holds(pane.id));
        let held: BTreeSet<PaneKind> = held.map(|pane| pane.kind).collect();
        assert_eq!(held, said.iter().copied().collect(), "{name}");
    }
    Ok(())
}
