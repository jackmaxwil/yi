#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

//! Headless Chrome for the console's browser pane: the installed Chrome or Chromium, else the
//! pinned headless shell, driven by CDP over `--remote-debugging-pipe`, frames as base64 PNG.

mod router;

pub use router::{Router, Update};

use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::os::fd::{AsRawFd, OwnedFd};
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Stdio};
use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

use rustix::fs::{Mode, OFlags};
use yi_types::cdp::CdpMessage;

const APPS: [&str; 2] = [
    "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
    "/Applications/Chromium.app/Contents/MacOS/Chromium",
];
const NAMES: [&str; 5] = [
    "google-chrome",
    "google-chrome-stable",
    "chromium",
    "chromium-browser",
    "chrome",
];
const FLAGS: [&str; 6] = [
    "--remote-debugging-pipe",
    "--headless=new",
    "--no-first-run",
    "--no-default-browser-check",
    "--hide-scrollbars",
    "--mute-audio",
];
/// Incident: the postmerge runner is root and runs the pinned headless shell, which Chrome
/// refuses to start as root with its sandbox on. Root drops it for that shell alone; a user's
/// installed Chrome keeps its sandbox around untrusted pages even under root.
fn flags(no_sandbox: bool) -> impl Iterator<Item = &'static str> {
    FLAGS
        .into_iter()
        .chain(no_sandbox.then_some("--no-sandbox"))
}

fn no_sandbox(root: bool, chrome: &Path, pinned: Option<&Path>) -> bool {
    root && Some(chrome) == pinned
}
/// A frame past this is drained to its NUL and refused; a 4K screencast PNG is about 12 MiB
/// of base64.
const MAX_FRAME: u64 = 32 << 20;

/// The configured path, else the first installed Chrome or Chromium, else `pinned`, where
/// `yi plugin add chrome` puts its headless shell; `Err` lists where it looked.
pub fn find(configured: Option<&Path>, pinned: Option<&Path>) -> Result<PathBuf, Vec<String>> {
    if let Some(path) = configured {
        if path.is_file() {
            return Ok(path.to_owned());
        }
        return Err(vec![path.display().to_string()]);
    }
    let path = std::env::var_os("PATH").unwrap_or_default();
    let dirs: Vec<PathBuf> = std::env::split_paths(&path)
        .filter(|dir| !dir.as_os_str().is_empty())
        .collect();
    let on_path = NAMES
        .iter()
        .flat_map(|name| dirs.iter().map(move |dir| dir.join(name)));
    let found = APPS
        .iter()
        .map(PathBuf::from)
        .chain(on_path)
        .chain(pinned.map(Path::to_owned))
        .find(|p| p.is_file());
    found.ok_or_else(|| {
        let mut looked: Vec<String> = APPS.iter().map(|app| (*app).to_owned()).collect();
        looked.push(format!("{} on PATH", NAMES.join(", ")));
        looked.extend(pinned.map(|path| path.display().to_string()));
        looked
    })
}

/// Runs `yi plugin add chrome`, the one verb that pins Chrome; its `error:` lines are the failure,
/// verbatim.
pub fn add_pinned(yi: &Path) -> Result<(), String> {
    #[expect(
        clippy::disallowed_methods,
        reason = "the console installs the pinned Chrome through the verb that pins it"
    )]
    let ran = std::process::Command::new(yi)
        .args(["plugin", "add", "chrome"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .output()
        .map_err(|error| format!("{}: {error}", yi.display()))?;
    if ran.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&ran.stderr);
    let errors: Vec<&str> = (stderr.lines())
        .filter(|line| line.starts_with("error:"))
        .collect();
    Err(match errors.is_empty() {
        true => format!("`yi plugin add chrome` exited {}", ran.status),
        false => errors.join(" · "),
    })
}

/// One `<pid>-<n>` directory per browser under here: two Chromes cannot share a profile.
pub fn profile_root(home: &Path) -> PathBuf {
    home.join(".yi/browser/profile")
}

/// A profile per browser, removed when it drops.
pub fn profile_dir() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let home = std::env::var_os("HOME").map_or_else(std::env::temp_dir, PathBuf::from);
    profile_root(&home).join(format!("{}-{n}", std::process::id()))
}

/// Incident: a crash or SIGHUP skips [`Browser`]'s drop and left its profile for good. True
/// when the pid in the name is gone; a recycled pid keeps one until it ends too.
pub fn profile_orphaned(dir: &Path) -> bool {
    let pid = dir
        .file_name()
        .and_then(OsStr::to_str)
        .and_then(|name| name.split_once('-'))
        .and_then(|(pid, _)| pid.parse::<i32>().ok())
        .and_then(rustix::process::Pid::from_raw);
    pid.is_some_and(|pid| rustix::process::test_kill_process(pid) == Err(rustix::io::Errno::SRCH))
}

pub struct Browser {
    pub router: Router,
    child: Child,
    inbox: Receiver<Result<CdpMessage, String>>,
    outbox: Option<SyncSender<Vec<u8>>>,
    profile: PathBuf,
}

impl Browser {
    /// Starts `chrome` through `shim __fds`, which hands it this pipe pair on fds 3 and 4.
    pub fn launch(
        shim: &Path,
        chrome: &Path,
        pinned: Option<&Path>,
        profile: &Path,
        url: &str,
        size: (u32, u32),
    ) -> std::io::Result<Self> {
        let mut profile_flag = OsString::from("--user-data-dir=");
        profile_flag.push(profile);
        #[expect(
            clippy::disallowed_methods,
            reason = "Chrome is the user's own program, started by the console that shows it"
        )]
        let mut child = std::process::Command::new(shim)
            .arg("__fds")
            .arg(chrome)
            .args(flags(no_sandbox(
                rustix::process::geteuid().is_root(),
                chrome,
                pinned,
            )))
            .arg(profile_flag)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;
        let (Some(mut stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            let _ = child.kill();
            let _ = child.wait();
            return Err(std::io::Error::other("Chrome's pipe was not opened"));
        };
        let (frames, inbox) = sync_channel(64);
        std::thread::spawn(move || read_frames(stdout, &frames));
        let (outbox, commands) = sync_channel::<Vec<u8>>(256);
        std::thread::spawn(move || {
            for bytes in commands {
                if stdin
                    .write_all(&bytes)
                    .and_then(|()| stdin.flush())
                    .is_err()
                {
                    break;
                }
            }
        });
        Ok(Self {
            router: Router::new(url, size),
            child,
            inbox,
            outbox: Some(outbox),
            profile: profile.to_owned(),
        })
    }

    pub fn poll(&mut self) -> Vec<Update> {
        let mut updates = Vec::new();
        while let Ok(message) = self.inbox.try_recv() {
            match message {
                Ok(message) => updates.extend(self.router.feed(message)),
                Err(why) => updates.push(Update::Failed(why)),
            }
        }
        for command in self.router.take() {
            let mut bytes = serde_json::to_vec(&command).unwrap_or_default();
            bytes.push(0);
            let sent = self
                .outbox
                .as_ref()
                .is_some_and(|out| out.try_send(bytes).is_ok());
            if !sent {
                updates.push(Update::Failed("Chrome stopped reading its pipe".to_owned()));
                break;
            }
        }
        updates
    }
}

impl Drop for Browser {
    fn drop(&mut self) {
        // Invariant: a closed pipe is Chrome's own signal to quit; the kill only backs it up.
        self.outbox = None;
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.profile);
    }
}

fn read_frames(stdout: impl Read, frames: &SyncSender<Result<CdpMessage, String>>) {
    let mut reader = BufReader::new(stdout);
    loop {
        let mut frame = Vec::new();
        let read = (&mut reader).take(MAX_FRAME).read_until(0, &mut frame);
        let (message, end) = match read {
            Ok(0) => (Err("Chrome closed its pipe".to_owned()), true),
            Err(error) => (Err(format!("reading Chrome's pipe: {error}")), true),
            Ok(_) if frame.last() == Some(&0) => {
                frame.pop();
                let parsed = serde_json::from_slice(&frame);
                (
                    parsed.map_err(|error| format!("an unreadable CDP frame: {error}")),
                    false,
                )
            }
            Ok(_) if u64::try_from(frame.len()).is_ok_and(|len| len < MAX_FRAME) => {
                (Err("Chrome closed its pipe mid-frame".to_owned()), true)
            }
            Ok(_) => {
                let mut rest = Vec::new();
                let drained = (&mut reader).take(MAX_FRAME).read_until(0, &mut rest);
                let end = drained.is_err() || rest.last() != Some(&0);
                (Err(format!("a CDP frame passed {MAX_FRAME} bytes")), end)
            }
        };
        if frames.send(message).is_err() || end {
            return;
        }
    }
}

/// `yi __fds <program> <args…>`: Chrome's pipe reads fd 3 and writes fd 4, and no `pre_exec`
/// runs under `forbid(unsafe_code)`, so this moves stdin and stdout there and execs the program.
pub fn exec_on_pipe_fds(args: &[OsString]) -> std::io::Error {
    let Some((program, rest)) = args.split_first() else {
        return std::io::Error::other("usage: yi __fds <program> [args…]");
    };
    let fds = match pipe_fds() {
        Ok(fds) => fds,
        Err(error) => return error,
    };
    #[expect(
        clippy::disallowed_methods,
        reason = "the exec replaces this shim with the program the browser pane asked for"
    )]
    let error = std::process::Command::new(program).args(rest).exec();
    drop(fds);
    error
}

fn pipe_fds() -> std::io::Result<[OwnedFd; 2]> {
    let null = || {
        let flags = OFlags::RDWR | OFlags::CLOEXEC;
        rustix::fs::open(OsStr::new("/dev/null"), flags, Mode::empty())
    };
    // Invariant: open takes the lowest free descriptor, so these are 3 and 4 unless one was
    // inherited; dup2 then clears close-on-exec on both.
    let (mut three, mut four) = (null()?, null()?);
    if three.as_raw_fd() != 3 || four.as_raw_fd() != 4 {
        return Err(std::io::Error::other(
            "file descriptors 3 and 4 were already open",
        ));
    }
    rustix::io::dup2(std::io::stdin(), &mut three)?;
    rustix::io::dup2(std::io::stdout(), &mut four)?;
    let quiet = null()?;
    rustix::stdio::dup2_stdin(&quiet)?;
    rustix::stdio::dup2_stdout(&quiet)?;
    Ok([three, four])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_pinned_headless_shell_drops_its_sandbox() {
        let pinned = Path::new("/root/.yi/plugins/chrome");
        assert!(
            no_sandbox(true, pinned, Some(pinned)),
            "the pinned headless shell drops its sandbox under root"
        );
        let installed = Path::new("/usr/bin/google-chrome");
        assert!(
            !no_sandbox(true, installed, Some(pinned)),
            "a root launch of an installed Chrome keeps its sandbox"
        );
        assert!(
            !no_sandbox(false, pinned, Some(pinned)),
            "a non-root launch keeps its sandbox even for the pinned shell"
        );
        assert!(!no_sandbox(true, pinned, None));
        assert!(flags(true).any(|flag| flag == "--no-sandbox"));
        assert!(flags(false).all(|flag| flag != "--no-sandbox"));
    }
}
