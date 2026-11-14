use std::io::Read;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use yi_types::encoding::base64;
use yi_types::image::image_defect;

pub const PNG: &str = "image/png";

const READ_DEADLINE: Duration = Duration::from_secs(5);

/// Incident: an unbounded `.output()` on a clipboard tool (a stuck `xclip` waiting on a
/// selection owner) hung the paste forever; the child is killed at [`READ_DEADLINE`].
fn run_bounded(program: &str, args: &[&str]) -> Option<Vec<u8>> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the clipboard is only readable through the OS's own tool"
    )]
    let mut command = Command::new(program);
    let mut child = command
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let mut stdout = child.stdout.take()?;
    let reader = std::thread::spawn(move || {
        let mut bytes = Vec::new();
        stdout.read_to_end(&mut bytes).map(|_| bytes)
    });
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if start.elapsed() < READ_DEADLINE => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    };
    let bytes = reader.join().ok()?.ok()?;
    status.success().then_some(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipImage {
    pub mime_type: String,
    pub data: String,
}

pub fn read_image() -> Result<Option<ClipImage>, String> {
    let Some(bytes) = read_png() else {
        return Ok(None);
    };
    let data = base64(&bytes);
    match image_defect(PNG, &data) {
        Some(defect) => Err(format!("the clipboard image {defect}")),
        None => Ok(Some(ClipImage {
            mime_type: PNG.to_owned(),
            data,
        })),
    }
}

#[cfg(target_os = "macos")]
fn read_png() -> Option<Vec<u8>> {
    let stdout = run_bounded("osascript", &["-e", "the clipboard as «class PNGf»"])?;
    let text = String::from_utf8(stdout).ok()?;
    let hex = text.trim().strip_prefix("«data PNGf")?.strip_suffix('»')?;
    hex.as_bytes()
        .chunks(2)
        .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).ok()?, 16).ok())
        .collect()
}

#[cfg(not(target_os = "macos"))]
fn read_png() -> Option<Vec<u8>> {
    let readers: [(&str, &[&str]); 2] = [
        ("wl-paste", &["--type", PNG]),
        ("xclip", &["-selection", "clipboard", "-t", PNG, "-o"]),
    ];
    readers
        .into_iter()
        .find_map(|(program, args)| run_bounded(program, args).filter(|bytes| !bytes.is_empty()))
}
