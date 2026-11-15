//! Daemon-handled config and app-launch requests (`_yi/settings`, `_yi/set_default`, `_yi/open`).
use std::process::Stdio;

use serde_json::{Value, json};

use super::{ClientId, Input, Supervisor, error_frame, result_frame};

impl Supervisor {
    /// The daemon answers for the config, not a worker: a worker holds the copy it started on.
    /// The read and fsync run off the routing loop, which every client and worker waits on.
    pub(super) fn settings(
        &mut self,
        client: ClientId,
        id: Option<Value>,
        method: &str,
        frame: &Value,
    ) {
        let Some(id) = id else { return };
        let home = self.options.home.clone();
        let save = method == "_yi/set_default";
        let setting = frame
            .pointer("/params/id")
            .and_then(Value::as_str)
            .map(str::to_owned);
        let value = frame.pointer("/params/value").cloned();
        let input = self.input.clone();
        // Incident: saves ran side by side, so a press could overwrite the newer one it raced.
        let before = self.settings_tail.take();
        let task = tokio::spawn(async move {
            if let Some(before) = before {
                let _ = before.await;
            }
            let _ = tokio::task::spawn_blocking(move || {
                let answer = match (save, setting, value) {
                    (false, _, _) => yi_runtime::settings::current(&home),
                    (true, Some(setting), Some(value)) => {
                        yi_runtime::settings::save(&home, &setting, value)
                    }
                    (true, _, _) => Err("a save names an id and a value; null unsets".to_owned()),
                };
                let line = match answer {
                    Ok(rows) => result_frame(id, json!({"rows": rows})),
                    Err(why) => error_frame(id, -32602, &why),
                };
                let _ = input.send(Input::Answer(client, line));
            })
            .await;
        });
        self.settings_tail = Some(task);
    }

    /// A message opened in an app the person listed: the daemon runs it, since the console never
    /// spawns, and only an app named in config.json can run.
    pub(super) fn open(&mut self, client: ClientId, id: Option<Value>, frame: &Value) {
        let Some(id) = id else { return };
        let home = self.options.home.clone();
        let name = frame
            .pointer("/params/name")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let text = frame
            .pointer("/params/text")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_owned();
        let input = self.input.clone();
        tokio::task::spawn_blocking(move || {
            let opened = yi_runtime::settings::open(&home, &name, &text).and_then(|argv| {
                spawn_app(&argv).map_err(|error| format!("{name}: {error}"))?;
                Ok(argv)
            });
            let line = match opened {
                Ok(argv) => result_frame(id, json!({"argv": argv})),
                Err(why) => error_frame(id, -32602, &why),
            };
            let _ = input.send(Input::Answer(client, line));
        });
    }
}

#[expect(
    clippy::disallowed_methods,
    reason = "an Open in app is one the person named in config.json; nothing else can be run"
)]
fn spawn_app(argv: &[String]) -> std::io::Result<()> {
    use std::os::unix::process::CommandExt;
    let (program, args) = argv.split_first().ok_or(std::io::ErrorKind::InvalidInput)?;
    let mut child = std::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn()?;
    // Invariant: a dropped Child is never reaped, so each launch would leave a zombie in the
    // long-lived daemon; a thread waits it out without holding the reply.
    std::thread::spawn(move || child.wait());
    Ok(())
}
