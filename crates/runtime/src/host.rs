//! Where this process runs, read once from files and environment variables, never the network.
use std::path::Path;

const CLOUDS: [(&str, &str); 6] = [
    ("Amazon EC2", "aws"),
    ("DigitalOcean", "digitalocean"),
    ("Google", "gcp"),
    ("Hetzner", "hetzner"),
    ("OpenStack", "openstack"),
    ("Scaleway", "scaleway"),
];
const HYPERVISORS: [&str; 8] = [
    "Alibaba",
    "KVM",
    "Parallels",
    "QEMU",
    "VMware",
    "Virtual Machine",
    "Xen",
    "innotek GmbH",
];
const RUNTIMES: [(&str, &str); 5] = [
    ("kubepods", "kubernetes"),
    ("containerd", "containerd"),
    ("libpod", "podman"),
    ("docker", "docker"),
    ("lxc", "lxc"),
];
const SSH_VARS: [&str; 3] = ["SSH_CONNECTION", "SSH_CLIENT", "SSH_TTY"];

/// Azure stamps every VM with this asset tag; its vendor string is Hyper-V's.
const AZURE_TAG: &str = "7783-7084-3265-9085-8269-3286-77";

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct HostFacts {
    pub container: Option<String>,
    pub vm: Option<String>,
    pub cloud: Option<String>,
    pub ssh: bool,
    pub wsl: bool,
}

impl HostFacts {
    pub fn line(&self) -> String {
        let mut parts = Vec::new();
        if let Some(container) = &self.container {
            parts.push(format!("{container} container"));
        }
        match (&self.cloud, &self.vm) {
            (Some(cloud), Some(vm)) => parts.push(format!("{cloud} vm ({vm})")),
            (Some(cloud), None) => parts.push(format!("{cloud} vm")),
            (None, Some(vm)) => parts.push(format!("vm ({vm})")),
            (None, None) => {}
        }
        if self.wsl {
            parts.push("wsl".to_owned());
        }
        if self.ssh {
            parts.push("over ssh".to_owned());
        }
        match parts.is_empty() {
            true => "local".to_owned(),
            false => parts.join(", "),
        }
    }
}

fn read(root: &Path, path: &str) -> Option<String> {
    std::fs::read_to_string(root.join(path))
        .ok()
        .map(|text| text.trim().to_owned())
}

fn dmi(root: &Path, field: &str) -> Option<String> {
    read(root, &format!("sys/class/dmi/id/{field}")).filter(|text| !text.is_empty())
}

fn container_of(root: &Path, env: &dyn Fn(&str) -> Option<String>) -> Option<String> {
    if env("KUBERNETES_SERVICE_HOST").is_some() {
        return Some("kubernetes".to_owned());
    }
    if let Some(runtime) = read(root, "proc/1/cgroup").and_then(|cgroup| {
        RUNTIMES
            .iter()
            .find(|(needle, _)| cgroup.contains(needle))
            .map(|(_, name)| (*name).to_owned())
    }) {
        return Some(runtime);
    }
    if root.join("run/.containerenv").exists() {
        return Some("podman".to_owned());
    }
    root.join(".dockerenv")
        .exists()
        .then(|| "docker".to_owned())
}

pub fn probe_at(root: &Path, env: &dyn Fn(&str) -> Option<String>) -> HostFacts {
    let vendor = dmi(root, "sys_vendor").unwrap_or_default();
    let product = dmi(root, "product_name").unwrap_or_default();
    let tag = dmi(root, "chassis_asset_tag").unwrap_or_default();
    let cloud = CLOUDS
        .iter()
        .find(|(needle, _)| vendor.contains(needle) || product.contains(needle))
        .map(|(_, name)| (*name).to_owned())
        .or_else(|| tag.contains(AZURE_TAG).then(|| "azure".to_owned()));
    let hypervisor = HYPERVISORS
        .iter()
        .find(|needle| vendor.contains(*needle) || product.contains(*needle))
        .map(|needle| match vendor.contains(needle) {
            true => vendor.clone(),
            false => product.clone(),
        });
    let flagged = read(root, "proc/cpuinfo")
        .is_some_and(|cpuinfo| cpuinfo.split_whitespace().any(|flag| flag == "hypervisor"));
    HostFacts {
        container: container_of(root, env),
        vm: hypervisor
            .or_else(|| {
                cloud
                    .as_ref()
                    .map(|_| vendor.clone())
                    .filter(|v| !v.is_empty())
            })
            .or_else(|| flagged.then(|| "a hypervisor".to_owned())),
        cloud,
        ssh: SSH_VARS
            .iter()
            .any(|key| env(key).is_some_and(|value| !value.is_empty())),
        wsl: read(root, "proc/sys/kernel/osrelease")
            .is_some_and(|release| release.to_lowercase().contains("microsoft")),
    }
}

/// The machine, probed once; the ssh fact is this process's own and is read every time, since
/// a daemon outlives the client that attached to it.
pub fn facts() -> HostFacts {
    static MACHINE: std::sync::OnceLock<HostFacts> = std::sync::OnceLock::new();
    let mut facts = MACHINE
        .get_or_init(|| {
            let mut facts = probe_at(Path::new("/"), &|key| std::env::var(key).ok());
            if facts.vm.is_none() && cfg!(target_os = "macos") && macos_guest() {
                facts.vm = Some("a hypervisor".to_owned());
            }
            facts
        })
        .clone();
    facts.ssh = SSH_VARS
        .iter()
        .any(|key| std::env::var(key).is_ok_and(|value| !value.is_empty()));
    facts
}

fn macos_guest() -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    let cancelled: yi_tools::CancelFlag =
        std::sync::Arc::new(move || std::time::Instant::now() >= deadline);
    let mut sysctl = yi_tools::command("/usr/sbin/sysctl");
    sysctl.args(["-n", "kern.hv_vmm_present"]);
    yi_tools::run_captured(sysctl, None, &cancelled, 64)
        .is_ok_and(|capture| capture.stdout.trim() == "1")
}
