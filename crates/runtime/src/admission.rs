//! Adaptive admission: a new child or lane is admitted while the host has room, never against a fixed count.

use std::fmt;
use std::num::NonZeroUsize;
use std::path::Path;
use std::time::Duration;
const LOAD_PER_CPU_MILLI: u64 = 4_000;
const MEM_FLOOR_KIB: u64 = 512 * 1024;
const MEM_PER_LIVE_KIB: u64 = 64 * 1024;
const DISK_FLOOR_KIB: u64 = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HostSample {
    pub cpus: NonZeroUsize,
    pub load1_milli: Option<u64>,
    pub mem_free_kib: Option<u64>,
    pub disk_free_kib: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resource {
    Cpu,
    Memory,
    Disk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Refusal {
    pub resource: Resource,
    pub have: u64,
    pub need: u64,
    pub live: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Admission {
    Admit,
    Refuse(Refusal),
}

impl fmt::Display for Refusal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let (what, have, need) = match self.resource {
            Resource::Cpu => (
                "load average x1000 (1 minute)",
                self.have.to_string(),
                format!("at most {}", self.need),
            ),
            Resource::Memory => (
                "free memory KiB",
                self.have.to_string(),
                format!("at least {}", self.need),
            ),
            Resource::Disk => (
                "free disk KiB on the lanes volume",
                self.have.to_string(),
                format!("at least {}", self.need),
            ),
        };
        write!(
            f,
            "the host has no room for another session: {what} is {have}, needs {need} with {} live. [admitted over refused by host load, memory and disk, no fixed count] wait for a child to finish or reap one with rlm.delete_subagent, then retry",
            self.live
        )
    }
}

pub fn admit(sample: &HostSample, live: usize) -> Admission {
    let load_cap = u64::try_from(sample.cpus.get())
        .unwrap_or(u64::MAX)
        .saturating_mul(LOAD_PER_CPU_MILLI);
    if let Some(load) = sample.load1_milli
        && live > 0
        && load > load_cap
    {
        return refuse(Resource::Cpu, load, load_cap, live);
    }
    let mem_need = MEM_FLOOR_KIB.saturating_add(
        u64::try_from(live)
            .unwrap_or(u64::MAX)
            .saturating_mul(MEM_PER_LIVE_KIB),
    );
    if let Some(free) = sample.mem_free_kib
        && free < mem_need
    {
        return refuse(Resource::Memory, free, mem_need, live);
    }
    if let Some(free) = sample.disk_free_kib
        && free < DISK_FLOOR_KIB
    {
        return refuse(Resource::Disk, free, DISK_FLOOR_KIB, live);
    }
    Admission::Admit
}

fn refuse(resource: Resource, have: u64, need: u64, live: usize) -> Admission {
    Admission::Refuse(Refusal {
        resource,
        have,
        need,
        live,
    })
}

pub fn sample(lanes_volume: &Path) -> HostSample {
    HostSample {
        cpus: std::thread::available_parallelism().unwrap_or(NonZeroUsize::MIN),
        load1_milli: load1_milli(),
        mem_free_kib: mem_free_kib(),
        disk_free_kib: disk_free_kib(lanes_volume),
    }
}

pub fn check(lanes_volume: &Path, live: usize) -> Result<(), Refusal> {
    match admit(&sample(lanes_volume), live) {
        Admission::Admit => Ok(()),
        Admission::Refuse(refusal) => Err(refusal),
    }
}

const PROBE_TIMEOUT: Duration = Duration::from_secs(10);

fn run(program: &str, args: &[&str]) -> Option<String> {
    crate::node::run(program, args, PROBE_TIMEOUT).ok()
}

fn load1_milli() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/loadavg")
        .ok()
        .or_else(|| run("sysctl", &["-n", "vm.loadavg"]))?;
    parse_load1_milli(&text)
}

pub(crate) fn parse_load1_milli(text: &str) -> Option<u64> {
    let first = text
        .split(|c: char| c.is_whitespace() || c == '{' || c == '}')
        .find(|token| !token.is_empty())?;
    let load: f64 = first.parse().ok()?;
    (load.is_finite() && load >= 0.0).then(|| (load * 1000.0).round() as u64)
}

fn mem_free_kib() -> Option<u64> {
    if let Ok(text) = std::fs::read_to_string("/proc/meminfo") {
        return parse_meminfo_available_kib(&text);
    }
    parse_vm_stat_free_kib(&run("vm_stat", &[])?)
}

pub(crate) fn parse_meminfo_available_kib(text: &str) -> Option<u64> {
    let line = text
        .lines()
        .find(|line| line.starts_with("MemAvailable:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}

pub(crate) fn parse_vm_stat_free_kib(text: &str) -> Option<u64> {
    let header = text.lines().next()?;
    let page_bytes: u64 = header
        .split("page size of ")
        .nth(1)?
        .split_whitespace()
        .next()?
        .parse()
        .ok()?;
    let pages = |name: &str| -> u64 {
        text.lines()
            .find_map(|line| line.strip_prefix(name))
            .and_then(|rest| rest.trim().trim_end_matches('.').parse().ok())
            .unwrap_or(0)
    };
    let total = pages("Pages free:")
        .saturating_add(pages("Pages inactive:"))
        .saturating_add(pages("Pages speculative:"));
    Some(total.saturating_mul(page_bytes) / 1024)
}

fn disk_free_kib(path: &Path) -> Option<u64> {
    let path = path.to_str()?;
    parse_df_available_kib(&run("df", &["-Pk", path])?)
}

pub(crate) fn parse_df_available_kib(text: &str) -> Option<u64> {
    text.lines().nth(1)?.split_whitespace().nth(3)?.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roomy() -> HostSample {
        HostSample {
            cpus: NonZeroUsize::new(8).unwrap_or(NonZeroUsize::MIN),
            load1_milli: Some(1_000),
            mem_free_kib: Some(16 * 1024 * 1024),
            disk_free_kib: Some(100 * 1024 * 1024),
        }
    }

    #[test]
    fn adaptive_admits_far_past_eight_while_the_host_has_room() {
        assert_eq!(admit(&roomy(), 200), Admission::Admit);
    }

    #[test]
    fn adaptive_refuses_on_load_and_names_the_numbers() -> Result<(), Box<dyn std::error::Error>> {
        let sample = HostSample {
            load1_milli: Some(40_000),
            ..roomy()
        };
        let Admission::Refuse(refusal) = admit(&sample, 3) else {
            return Err("expected a refusal".into());
        };
        assert_eq!(refusal.resource, Resource::Cpu);
        assert_eq!((refusal.have, refusal.need), (40_000, 32_000));
        let text = refusal.to_string();
        assert!(text.contains("40000") && text.contains("32000") && text.contains("3 live"));
        Ok(())
    }

    #[test]
    fn adaptive_first_child_ignores_load_so_work_can_always_start() {
        let sample = HostSample {
            load1_milli: Some(900_000),
            ..roomy()
        };
        assert_eq!(admit(&sample, 0), Admission::Admit);
    }

    #[test]
    fn adaptive_memory_headroom_grows_with_live_children() -> Result<(), Box<dyn std::error::Error>>
    {
        let sample = HostSample {
            mem_free_kib: Some(MEM_FLOOR_KIB + 5 * MEM_PER_LIVE_KIB),
            ..roomy()
        };
        assert_eq!(admit(&sample, 5), Admission::Admit);
        let Admission::Refuse(refusal) = admit(&sample, 6) else {
            return Err("expected a refusal".into());
        };
        assert_eq!(refusal.resource, Resource::Memory);
        assert_eq!(refusal.need, MEM_FLOOR_KIB + 6 * MEM_PER_LIVE_KIB);
        Ok(())
    }

    #[test]
    fn adaptive_refuses_on_low_disk() -> Result<(), Box<dyn std::error::Error>> {
        let sample = HostSample {
            disk_free_kib: Some(DISK_FLOOR_KIB - 1),
            ..roomy()
        };
        let Admission::Refuse(refusal) = admit(&sample, 1) else {
            return Err("expected a refusal".into());
        };
        assert_eq!(refusal.resource, Resource::Disk);
        Ok(())
    }

    #[test]
    fn adaptive_unreadable_resources_do_not_block_admission() {
        let sample = HostSample {
            load1_milli: None,
            mem_free_kib: None,
            disk_free_kib: None,
            ..roomy()
        };
        assert_eq!(admit(&sample, 50), Admission::Admit);
    }

    #[test]
    fn adaptive_parsers_read_real_producer_output() {
        assert_eq!(parse_load1_milli("{ 2.34 1.80 1.51 }\n"), Some(2_340));
        assert_eq!(parse_load1_milli("0.05 0.10 0.20 1/300 999\n"), Some(50));
        assert_eq!(parse_load1_milli(""), None);
        assert_eq!(
            parse_meminfo_available_kib("MemTotal: 1 kB\nMemAvailable:   4096 kB\n"),
            Some(4096)
        );
        let vm_stat = "Mach Virtual Memory Statistics: (page size of 16384 bytes)\nPages free:                               100.\nPages active:                             999.\nPages inactive:                            50.\nPages speculative:                         10.\n";
        assert_eq!(parse_vm_stat_free_kib(vm_stat), Some(160 * 16));
        let df = "Filesystem 1024-blocks Used Available Capacity Mounted on\n/dev/disk3s5 971350180 500 123456 50% /\n";
        assert_eq!(parse_df_available_kib(df), Some(123_456));
    }

    #[test]
    fn adaptive_sampling_this_host_reads_at_least_disk() {
        let sample = sample(&std::env::temp_dir());
        assert!(sample.disk_free_kib.is_some());
    }
}
