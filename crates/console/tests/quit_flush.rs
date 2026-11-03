//! A request queued just before quit still reaches the daemon.

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixListener;
use std::time::Duration;

use serde_json::json;
use yi_console::client::{self, ClientEvent};

#[test]
fn a_frame_queued_before_shutdown_is_written_even_if_the_daemon_speaks_after()
-> Result<(), Box<dyn std::error::Error>> {
    let dir = std::env::temp_dir().join(format!("yi-qf-{}", std::process::id()));
    std::fs::create_dir_all(&dir)?;
    for round in 0..200 {
        let socket = dir.join(format!("{round}.sock"));
        let listener = UnixListener::bind(&socket)?;
        let (events, received) = client::events();
        let (outbound, threads) = client::spawn(socket, events);
        let (mut daemon, _) = listener.accept()?;
        while !matches!(
            received.recv_timeout(Duration::from_secs(5))?,
            ClientEvent::Connected
        ) {}
        assert!(outbound.send(&json!({"id": round})));
        outbound.shutdown();
        daemon.write_all(b"{}\n")?;
        let mut line = String::new();
        BufReader::new(&daemon).read_line(&mut line)?;
        assert_eq!(line.trim(), format!("{{\"id\":{round}}}"), "round {round}");
        drop(threads);
    }
    Ok(())
}
