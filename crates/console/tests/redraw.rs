//! A frame is drawn for what can be seen: an agent event for a session no pane shows
//! changes nothing on screen.

use serde_json::{Value, json};
use yi_console::app::App;
use yi_console::client::{self, ClientEvent};
use yi_tui::colors::{ColorTier, Theme};
use yi_types::event::{AgentEvent, AssistantMessageEvent};

fn update(update: Value) -> ClientEvent {
    ClientEvent::Frame(json!({
        "jsonrpc": "2.0",
        "method": "session/update",
        "params": {"sessionId": "s-background", "update": update},
    }))
}

#[test]
fn a_background_session_streaming_asks_for_no_frame() {
    let (events, _received) = client::events();
    let socket = std::env::temp_dir().join("yi-redraw-no-daemon.sock");
    let (outbound, _threads) = client::spawn(socket, events);
    let mut app = App::new(
        "/tmp/demo-root".to_owned(),
        Theme::new(ColorTier::Ansi16, true),
    );
    let delta = AgentEvent::MessageUpdate {
        assistant_message_event: AssistantMessageEvent::TextDelta {
            content_index: 0,
            delta: "hidden".to_owned(),
        },
    };
    for seq in 1..=3 {
        app.dirty = false;
        app.reduce_client(
            &outbound,
            update(json!({"sessionUpdate": "_yi/event", "event": delta, "seq": seq})),
        );
        assert!(
            !app.dirty,
            "delta {seq} of a session no pane shows asked for a frame"
        );
    }
    app.reduce_client(
        &outbound,
        update(json!({"sessionUpdate": "state_update", "state": "running"})),
    );
    assert!(app.dirty, "its rail row still redraws");
    outbound.shutdown();
}
