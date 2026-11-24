//! The machine's own short name, read once a run with `/bin/hostname`.

use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::thread;
use std::time::{Duration, Instant};

/// The longest `hostname` may take before it is killed and the name left out.
const TIMEOUT: Duration = Duration::from_secs(2);

static NAME: OnceLock<Option<String>> = OnceLock::new();

/// What `hostname -s` printed, or none when it could not say.
pub(crate) fn name() -> Option<&'static str> {
    NAME.get_or_init(read).as_deref()
}

fn read() -> Option<String> {
    #[allow(clippy::disallowed_methods, reason = "the hostname spawn")]
    let mut child = Command::new("/bin/hostname")
        .arg("-s")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let due = Instant::now().checked_add(TIMEOUT)?;
    while child.try_wait().ok()?.is_none() {
        if Instant::now() > due {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(5));
    }
    let mut out = String::new();
    std::io::Read::read_to_string(&mut child.stdout.take()?, &mut out).ok()?;
    let name = out.trim();
    (!name.is_empty()).then(|| name.chars().take(64).collect())
}
