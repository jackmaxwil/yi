//! The node card and what it admits (plan sections 3.4 and 8.1): the kernels one machine
//! holds live at once, and the container a child's tool calls run in.

use std::fs::File;
use std::io::{Read, Write};
use std::num::{NonZeroU8, NonZeroUsize};
use std::path::Path;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use yi_types::config::NodeConfig;
use yi_types::node::{NodeCapacity, NodeCard};

const WORKTREE: &str = "worktree";
const CONTAINER: &str = "container";
const PROBE: Duration = Duration::from_secs(10);
const PULL: Duration = Duration::from_secs(600);
const START: Duration = Duration::from_secs(120);
const WAIT_POLL: Duration = Duration::from_millis(200);

static CONFIG: OnceLock<NodeConfig> = OnceLock::new();

/// The config's `node`, once per process before any kernel boots; unset reads the card alone.
pub fn configure(config: NodeConfig) {
    let _first_wins = CONFIG.set(config);
}

fn run(program: &str, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut command = yi_tools::command(program);
    command.args(args);
    let deadline = Instant::now().checked_add(timeout);
    let cancelled: yi_tools::CancelFlag =
        Arc::new(move || deadline.is_some_and(|at| Instant::now() >= at));
    let shown = format!("{program} {}", args.join(" "));
    let capture = yi_tools::run_captured(command, None, &cancelled, 16_384)
        .map_err(|error| format!("`{shown}`: {error}"))?;
    match capture.exit_code {
        Some(0) => Ok(capture.stdout),
        code => Err(format!(
            "`{shown}` exited {}: {}",
            code.map_or_else(
                || "on a signal or its timeout".to_owned(),
                |c| c.to_string()
            ),
            capture.stderr.trim()
        )),
    }
}

fn memory_gb() -> u64 {
    let linux = std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|text| {
            let kib = text
                .lines()
                .find_map(|line| line.strip_prefix("MemTotal:"))?;
            let kib: u64 = kib.trim().trim_end_matches("kB").trim().parse().ok()?;
            Some(kib.saturating_mul(1024))
        });
    let bytes = linux.or_else(|| {
        run("sysctl", &["-n", "hw.memsize"], PROBE)
            .ok()
            .and_then(|text| text.trim().parse().ok())
    });
    bytes.unwrap_or(0) >> 30
}

/// The card a machine computes for itself: the cores less one as slots, 1 to 8.
pub fn computed(cpus: usize, mem_gb: u64, docker: bool, name: String) -> NodeCard {
    let slots = u8::try_from(cpus.saturating_sub(1).clamp(1, 8))
        .ok()
        .and_then(NonZeroU8::new)
        .unwrap_or(NonZeroU8::MIN);
    let mut isolation = vec![WORKTREE.to_owned()];
    if docker {
        isolation.push(CONTAINER.to_owned());
    }
    NodeCard {
        name,
        always_on: false,
        power: "unknown".to_owned(),
        slots,
        capacity: NodeCapacity {
            cpus: u32::try_from(cpus).unwrap_or(u32::MAX),
            mem_gb,
            extra: Default::default(),
        },
        isolation,
        price_per_hour: 0.into(),
        extra: Default::default(),
    }
}

pub fn overridden(mut card: NodeCard, config: &NodeConfig) -> NodeCard {
    if let Some(slots) = config.slots {
        card.slots = slots;
    }
    if let Some(isolation) = &config.isolation {
        card.isolation.clone_from(isolation);
    }
    card
}

/// `~/.yi/node.json` as written, computed and written on first use; a card that does not
/// parse is an error naming the file, never a silent recompute over the owner's edit.
pub fn card(home: &Path) -> Result<NodeCard, String> {
    let path = home.join(".yi/node.json");
    let card = match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| format!("{}: {error}", path.display()))?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let cpus = std::thread::available_parallelism().map_or(1, NonZeroUsize::get);
            let docker = run(
                "docker",
                &["version", "--format", "{{.Server.Version}}"],
                PROBE,
            );
            let name = run("hostname", &[], PROBE)
                .map_or_else(|_| "local".to_owned(), |name| name.trim().to_owned());
            let card = computed(cpus, memory_gb(), docker.is_ok(), name);
            let bytes = serde_json::to_vec_pretty(&card).map_err(|error| error.to_string())?;
            // Invariant: renamed into place, so a second process reads a whole card or none.
            let staged = path.with_extension(format!("json.{}", std::process::id()));
            std::fs::create_dir_all(home.join(".yi"))
                .and_then(|()| std::fs::write(&staged, bytes))
                .and_then(|()| std::fs::rename(&staged, &path))
                .map_err(|error| format!("{}: {error}", path.display()))?;
            card
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    Ok(match CONFIG.get() {
        Some(config) => overridden(card, config),
        None => card,
    })
}

/// A slot held for one kernel: the flock goes with the file, so a process that dies, however
/// it dies, drops it, and `waited` is the notice a wait for it left, for the call that booted.
pub struct Admitted {
    pub slot: File,
    pub waited: Option<String>,
}

fn full(slots: NonZeroU8, holders: &[String]) -> String {
    format!(
        "node: {n} of {n} slots held (node.slots={n}); this kernel waits for one. Held by {}. A reaped child (rlm.delete_subagent) or an ended session frees one; node.slots in config raises the bound.",
        holders.join("; "),
        n = slots
    )
}

/// Every live kernel on the machine takes a slot, whichever process owns it; past `slots` a
/// boot waits here, saying so through `progress` once per change in who holds them.
pub async fn admit(
    home: &Path,
    holder: &str,
    progress: &(dyn Fn(&str) + Send + Sync),
    cancelled: &yi_tools::CancelFlag,
) -> Result<Admitted, String> {
    let card = card(home)?;
    let dir = home.join(".yi/node/slots");
    std::fs::create_dir_all(&dir).map_err(|error| format!("{}: {error}", dir.display()))?;
    let started = Instant::now();
    let mut told: Option<String> = None;
    loop {
        let mut holders = Vec::new();
        for index in 0..card.slots.get() {
            let path = dir.join(format!("{index}.held"));
            let io = |error: std::io::Error| format!("{}: {error}", path.display());
            let mut file = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(&path)
                .map_err(io)?;
            match file.try_lock() {
                Ok(()) => {
                    file.set_len(0)
                        .and_then(|()| file.write_all(holder.as_bytes()))
                        .map_err(io)?;
                    let waited = told.map(|notice| {
                        format!(
                            "[{notice} It waited {:.1} s and now holds slot {index}.]",
                            started.elapsed().as_secs_f64()
                        )
                    });
                    return Ok(Admitted { slot: file, waited });
                }
                Err(std::fs::TryLockError::WouldBlock) => {
                    let mut text = String::new();
                    let _a_holder_mid_write_reads_blank = file.read_to_string(&mut text);
                    holders.push(format!("slot {index}: {}", text.trim()));
                }
                Err(std::fs::TryLockError::Error(error)) => return Err(io(error)),
            }
        }
        let notice = full(card.slots, &holders);
        if told.as_ref() != Some(&notice) {
            progress(&notice);
        }
        if cancelled() {
            return Err(format!("{notice} It stopped waiting: cancelled."));
        }
        told = Some(notice);
        tokio::time::sleep(WAIT_POLL).await;
    }
}

/// `container:<image>` names an image docker can take as one argument, never a flag.
pub fn image_of(tag: &str) -> Option<Result<&str, String>> {
    let image = tag.strip_prefix("container:")?;
    let valid = !image.is_empty()
        && !image.starts_with('-')
        && image
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@'));
    Some(match valid {
        true => Ok(image),
        false => Err(format!(
            "isolation {tag:?} names no image: container:<image> takes letters, digits and . _ - / : @"
        )),
    })
}

/// One long-lived container per child over its lane, bind-mounted at the same path; removed
/// when its record drops, so a reap, a repossession and a spawn that failed after it all remove it.
#[derive(Debug)]
pub struct Container {
    name: String,
}

/// Refused before a lane is claimed: a node whose card lacks `container` places nothing.
pub fn placeable(home: &Path, image: &str) -> Result<(), String> {
    let card = card(home)?;
    if card.isolation.iter().any(|kind| kind == CONTAINER) {
        return Ok(());
    }
    Err(format!(
        "container:{image} needs docker, and this node has no container isolation (node.isolation={:?} in {}); start docker, then delete that file to recompute it or set node.isolation in config",
        card.isolation,
        home.join(".yi/node.json").display()
    ))
}

impl Container {
    pub fn up(image: &str, lane: &Path, notice: &dyn Fn(&str)) -> Result<Self, String> {
        if run(
            "docker",
            &["image", "inspect", "--format", "{{.Id}}", image],
            PROBE,
        )
        .is_err()
        {
            notice(&format!(
                "[node: pulling {image}: it is not on this node, and the spawn waits for the pull]"
            ));
            run("docker", &["pull", image], PULL)?;
        }
        let lane_text = lane.to_string_lossy().into_owned();
        let hash = crate::ext::content_hash(&lane_text);
        let name = format!("yi-{}", hash.chars().take(12).collect::<String>());
        // A host that died with a child live left its container here; the lane's name reclaims it.
        let _none_left_is_fine = run("docker", &["rm", "-f", &name], PROBE);
        let mut args: Vec<String> = ["run", "-d", "--rm", "--name", &name, "--label"]
            .map(str::to_owned)
            .to_vec();
        args.push(format!("yi.lane={lane_text}"));
        #[cfg(unix)]
        if let Ok(meta) = std::fs::metadata(lane) {
            use std::os::unix::fs::MetadataExt;
            args.extend([
                "--user".to_owned(),
                format!("{}:{}", meta.uid(), meta.gid()),
            ]);
        }
        args.extend(["-e", "HOME=/tmp", "-w", &lane_text].map(str::to_owned));
        let mut mounts = vec![lane.to_path_buf()];
        mounts.extend(yi_permission::git_dirs(lane));
        for mount in mounts {
            let mount = mount.to_string_lossy();
            args.extend(["-v".to_owned(), format!("{mount}:{mount}")]);
        }
        args.extend(["--entrypoint", "tail", image, "-f", "/dev/null"].map(str::to_owned));
        let args: Vec<&str> = args.iter().map(String::as_str).collect();
        run("docker", &args, START)?;
        Ok(Self { name })
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for Container {
    fn drop(&mut self) {
        let _gone_already_is_fine = run("docker", &["rm", "-f", &self.name], PROBE);
    }
}
