#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

use std::collections::BTreeMap;
use std::error::Error;

use yi_runtime::host::{HostFacts, probe_at};

type TestResult = Result<(), Box<dyn Error>>;

fn env(pairs: &[(&str, &str)]) -> impl Fn(&str) -> Option<String> {
    let map: BTreeMap<String, String> = pairs
        .iter()
        .map(|(key, value)| ((*key).to_owned(), (*value).to_owned()))
        .collect();
    move |key: &str| map.get(key).cloned()
}

fn write(root: &std::path::Path, path: &str, text: &str) -> TestResult {
    let file = root.join(path);
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(file, text)?;
    Ok(())
}

#[test]
fn a_bare_host_reads_local() -> TestResult {
    let root = Scratch::new("yi-host-bare")?;
    let facts = probe_at(&root, &env(&[]));
    assert_eq!(facts, HostFacts::default(), "{facts:?}");
    assert_eq!(facts.line(), "local");
    Ok(())
}

#[test]
fn host_probe_reads_container_vm_cloud_ssh_and_wsl() -> TestResult {
    let docker = Scratch::new("yi-host-docker")?;
    write(&docker, ".dockerenv", "")?;
    let facts = probe_at(
        &docker,
        &env(&[("SSH_CONNECTION", "203.0.113.4 22 203.0.113.9 22")]),
    );
    assert_eq!(facts.container.as_deref(), Some("docker"));
    assert!(facts.ssh);
    assert_eq!(facts.line(), "docker container, over ssh");

    let cgroup = Scratch::new("yi-host-cgroup")?;
    write(&cgroup, "proc/1/cgroup", "0::/kubepods/besteffort/pod123\n")?;
    let facts = probe_at(&cgroup, &env(&[("KUBERNETES_SERVICE_HOST", "203.0.113.1")]));
    assert_eq!(facts.container.as_deref(), Some("kubernetes"));

    let podman = Scratch::new("yi-host-podman")?;
    write(&podman, "run/.containerenv", "")?;
    assert_eq!(
        probe_at(&podman, &env(&[])).container.as_deref(),
        Some("podman")
    );

    let ec2 = Scratch::new("yi-host-ec2")?;
    write(&ec2, "sys/class/dmi/id/sys_vendor", "Amazon EC2\n")?;
    write(&ec2, "sys/class/dmi/id/product_name", "t3.medium\n")?;
    let facts = probe_at(&ec2, &env(&[]));
    assert_eq!(facts.cloud.as_deref(), Some("aws"));
    assert_eq!(facts.vm.as_deref(), Some("Amazon EC2"));
    assert_eq!(facts.line(), "aws vm (Amazon EC2)");

    let azure = Scratch::new("yi-host-azure")?;
    write(
        &azure,
        "sys/class/dmi/id/sys_vendor",
        "Microsoft Corporation\n",
    )?;
    write(&azure, "sys/class/dmi/id/product_name", "Virtual Machine\n")?;
    write(
        &azure,
        "sys/class/dmi/id/chassis_asset_tag",
        "7783-7084-3265-9085-8269-3286-77\n",
    )?;
    assert_eq!(probe_at(&azure, &env(&[])).cloud.as_deref(), Some("azure"));

    let qemu = Scratch::new("yi-host-qemu")?;
    write(&qemu, "sys/class/dmi/id/sys_vendor", "QEMU\n")?;
    let facts = probe_at(&qemu, &env(&[]));
    assert_eq!(facts.vm.as_deref(), Some("QEMU"));
    assert!(facts.cloud.is_none(), "a local hypervisor is no cloud");

    let wsl = Scratch::new("yi-host-wsl")?;
    write(
        &wsl,
        "proc/sys/kernel/osrelease",
        "5.15.0-microsoft-standard-WSL2\n",
    )?;
    let facts = probe_at(&wsl, &env(&[]));
    assert!(facts.wsl);
    assert_eq!(facts.line(), "wsl");

    let hypervisor = Scratch::new("yi-host-flag")?;
    write(
        &hypervisor,
        "proc/cpuinfo",
        "flags\t: fpu vme hypervisor lm\n",
    )?;
    assert_eq!(
        probe_at(&hypervisor, &env(&[])).vm.as_deref(),
        Some("a hypervisor")
    );
    Ok(())
}
