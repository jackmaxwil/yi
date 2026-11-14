use std::error::Error;
use std::path::PathBuf;
use std::process::Output;

use serde_json::Value;

use crate::scratch::Scratch;
use crate::yi_bin::yi_command;

pub(crate) struct Workspace(pub(crate) Scratch);

impl Workspace {
    pub(crate) fn new(tag: &str) -> Result<Self, Box<dyn Error>> {
        let dir = Scratch::new(&format!("yi-cli-{tag}"))?;
        std::fs::create_dir_all(dir.join("project"))?;
        dir.home()?;
        Ok(Self(dir))
    }

    pub(crate) fn project(&self) -> PathBuf {
        self.0.join("project")
    }

    pub(crate) fn yi(&self, args: &[&str]) -> Result<Output, Box<dyn Error>> {
        self.yi_env(args, &[])
    }

    pub(crate) fn yi_env(
        &self,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<Output, Box<dyn Error>> {
        let mut command = yi_command(&self.0.join("home"));
        command
            .args(args)
            .arg("--session-dir")
            .arg(self.0.join("home/sessions"))
            .arg("--cwd")
            .arg(self.project());
        // Incident: a shell's OPENROUTER_API_KEY turned the keyless doctor FAIL into a live probe.
        for (_, key) in yi_types::PROVIDER_KEY_VARS {
            command.env_remove(key);
        }
        command
            .envs(env.iter().copied())
            .current_dir(self.project());
        Ok(command.output()?)
    }

    /// `yi plugin` takes no session flags, and PATH holds no ripwire the test did not place.
    pub(crate) fn plugin(&self, args: &[&str]) -> std::io::Result<Output> {
        let mut command = yi_command(&self.0.join("home"));
        command
            .arg("plugin")
            .args(args)
            .env("PATH", "/usr/bin:/bin")
            .output()
    }

    /// One `yi acp` process fed every frame at once; the contract is the response stream.
    pub(crate) fn acp(
        &self,
        args: &[&str],
        frames: &[Value],
    ) -> Result<Vec<Value>, Box<dyn Error>> {
        use std::io::Write;
        use std::process::Stdio;
        let mut command = yi_command(&self.0.join("home"));
        command
            .arg("acp")
            .args(args)
            .arg("--session-dir")
            .arg(self.0.join("home/sessions"))
            .arg("--cwd")
            .arg(self.project())
            .current_dir(self.project())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null());
        for (_, key) in yi_types::PROVIDER_KEY_VARS {
            command.env_remove(key);
        }
        let mut child = command.spawn()?;
        {
            let mut stdin = child.stdin.take().ok_or("acp has no stdin")?;
            for frame in frames {
                serde_json::to_writer(&mut stdin, frame)?;
                stdin.write_all(b"\n")?;
            }
        }
        let output = child.wait_with_output()?;
        let mut responses = Vec::new();
        let text = String::from_utf8_lossy(&output.stdout).into_owned();
        for line in text.lines() {
            responses.push(serde_json::from_str::<Value>(line)?);
        }
        Ok(responses)
    }
}
