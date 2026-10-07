use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use yi_runtime::tarball;
use yi_types::install::InstallReceipt;

const FORGE: &str = "https://git.example.invalid";
const NAMESPACE: &str = "yi-release";
const SIGNERS: &str = include_str!("../../../allowed_signers");

static TICK: AtomicU64 = AtomicU64::new(0);

pub(crate) fn early() {
    if std::env::args().nth(1).as_deref() != Some("update") {
        return;
    }
    std::process::exit(run());
}

fn run() -> i32 {
    if std::env::args().nth(2).is_some() {
        eprintln!("usage: yi update");
        return 2;
    }
    match installed() {
        Ok(line) => {
            println!("{line}");
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn installed() -> Result<String, String> {
    let home = absolute_home()?;
    let prefix = prefix_dir(&home)?;
    let exe = std::env::current_exe().map_err(|error| format!("current exe: {error}"))?;
    apply(Apply {
        home: &home,
        prefix: &prefix,
        exe: &exe,
        current: env!("ARCHITECTURE_VERSION"),
        target: env!("RELEASE_TRIPLE"),
        signers: SIGNERS,
        get: &http_get,
    })
}

struct Apply<'a, F> {
    home: &'a Path,
    prefix: &'a Path,
    exe: &'a Path,
    current: &'a str,
    target: &'a str,
    signers: &'a str,
    get: &'a F,
}

struct Release {
    version: String,
    archive: String,
    digest: String,
    signature: String,
}

fn apply<F>(job: Apply<'_, F>) -> Result<String, String>
where
    F: Fn(&str) -> Result<Vec<u8>, String>,
{
    let receipt = read_receipt(job.home)?;
    let dest = installed_binary(job.prefix, receipt.as_ref());
    guard(job.exe, job.prefix, receipt.as_ref(), &dest)?;
    let body = (job.get)(&format!("{FORGE}/api/v1/repos/apex/yi/releases/latest"))?;
    let latest: serde_json::Value =
        serde_json::from_slice(&body).map_err(|error| error.to_string())?;
    let release = release_from(&latest, job.target)?;
    if release.version == job.current {
        return Ok(format!("already {}", job.current));
    }
    let archive = (job.get)(&release.archive)?;
    let digest = (job.get)(&release.digest)?;
    let got = tarball::sha256_hex(&archive);
    let want = digest_hex(&digest)?;
    if !got.eq_ignore_ascii_case(&want) {
        return Err("the release digest does not match".to_owned());
    }
    let signature = (job.get)(&release.signature)?;
    verify_signature(job.home, &archive, &signature, job.signers)?;
    let unpacked = unpack(&archive)?;
    let mut next = receipt.unwrap_or(InstallReceipt {
        prefix: String::new(),
        version: String::new(),
        target: String::new(),
        extra: std::collections::BTreeMap::new(),
    });
    let prefix = dest
        .parent()
        .ok_or_else(|| format!("{} has no parent", dest.display()))?;
    next.prefix = prefix.display().to_string();
    next.version.clone_from(&release.version);
    next.target = job.target.to_owned();
    publish(&dest, &unpacked.binary, job.home, &unpacked.skills)?;
    write_receipt(job.home, &next)?;
    Ok(format!(
        "installed {} → {} at {}",
        job.current,
        release.version,
        prefix.display()
    ))
}

fn absolute_home() -> Result<PathBuf, String> {
    let home = PathBuf::from(
        std::env::var_os("HOME").ok_or("HOME is not set; yi needs an absolute HOME")?,
    );
    if !home.is_absolute() {
        return Err(format!(
            "HOME is relative ({}); yi needs an absolute HOME",
            home.display()
        ));
    }
    Ok(home)
}

fn prefix_dir(home: &Path) -> Result<PathBuf, String> {
    match std::env::var_os("YI_PREFIX") {
        Some(value) => {
            let path = PathBuf::from(value);
            if !path.is_absolute() {
                return Err(format!(
                    "YI_PREFIX is relative ({}); yi needs an absolute prefix",
                    path.display()
                ));
            }
            Ok(path)
        }
        None => Ok(home.join(".local/bin")),
    }
}

fn installed_binary(prefix: &Path, receipt: Option<&InstallReceipt>) -> PathBuf {
    match receipt {
        Some(receipt) => PathBuf::from(&receipt.prefix).join("yi"),
        None => prefix.join("yi"),
    }
}

fn guard(
    exe: &Path,
    prefix: &Path,
    receipt: Option<&InstallReceipt>,
    dest: &Path,
) -> Result<(), String> {
    let refuse = || format!("{} is not the installed binary", exe.display());
    let meta =
        std::fs::symlink_metadata(exe).map_err(|error| format!("{}: {error}", exe.display()))?;
    if !meta.file_type().is_file() {
        return Err(refuse());
    }
    if !same_file(exe, dest) {
        return Err(refuse());
    }
    if receipt.is_none() {
        let named = exe.file_name().is_some_and(|name| name == "yi");
        let direct = exe.parent().is_some_and(|parent| same_file(parent, prefix));
        if !named || !direct {
            return Err(refuse());
        }
    }
    Ok(())
}

fn same_file(left: &Path, right: &Path) -> bool {
    match (std::fs::canonicalize(left), std::fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn read_receipt(home: &Path) -> Result<Option<InstallReceipt>, String> {
    let path = home.join(".yi/install.json");
    match std::fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| format!("{}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("{}: {error}", path.display())),
    }
}

fn release_from(body: &serde_json::Value, target: &str) -> Result<Release, String> {
    let tag = body
        .get("tag_name")
        .and_then(|value| value.as_str())
        .ok_or("the release names no version")?;
    let version = tag.strip_prefix('v').unwrap_or(tag);
    let name = format!("yi-{version}-{target}.tar.gz");
    let assets = body
        .get("assets")
        .and_then(|value| value.as_array())
        .ok_or("the release lists no assets")?;
    let url = |suffix: &str| -> Result<String, String> {
        let want = format!("{name}{suffix}");
        assets
            .iter()
            .find_map(|asset| {
                let found = asset.get("name").and_then(|value| value.as_str())?;
                if found != want {
                    return None;
                }
                asset
                    .get("browser_download_url")
                    .and_then(|value| value.as_str())
                    .map(str::to_owned)
            })
            .ok_or_else(|| format!("the release has no {want}"))
    };
    Ok(Release {
        version: version.to_owned(),
        archive: url("")?,
        digest: url(".sha256")?,
        signature: url(".sig")?,
    })
}

fn http_get(url: &str) -> Result<Vec<u8>, String> {
    let error = match fetch(url, &agent()) {
        Ok(bytes) => return Ok(bytes),
        Err(error) => error,
    };
    if !error.contains("UnknownIssuer") {
        return Err(error);
    }
    let agent = match login_agent() {
        Ok(agent) => agent,
        Err(cause) => return Err(format!("{error} ({cause})")),
    };
    fetch(url, &agent)
}

fn agent() -> ureq::Agent {
    tarball::agent().build()
}

fn on_forge(url: &str) -> bool {
    url.strip_prefix(FORGE)
        .is_some_and(|rest| rest.starts_with('/'))
}

fn fetch(url: &str, agent: &ureq::Agent) -> Result<Vec<u8>, String> {
    let request = agent.get(url);
    let request = match token_in(&fgj_config()).filter(|_| on_forge(url)) {
        Some(token) => request.set("Authorization", &format!("token {token}")),
        None => request,
    };
    let response = match request.call() {
        Ok(response) => response,
        Err(ureq::Error::Status(401 | 403 | 404, _)) => {
            return Err("the release is not readable".to_owned());
        }
        Err(error) => return Err(error.to_string()),
    };
    tarball::read_capped(response.into_reader())
}

#[cfg(target_os = "macos")]
fn login_keychain() -> Result<String, String> {
    let user = std::env::var("USER").map_err(|_| "USER is unset".to_owned())?;
    #[expect(
        clippy::disallowed_methods,
        reason = "HOME may be a prefix under test; the keychain lives in the account home"
    )]
    let output = std::process::Command::new("dscl")
        .args([".", "-read", &format!("/Users/{user}"), "NFSHomeDirectory"])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let home = text
        .split_once(':')
        .map(|(_, rest)| rest.trim())
        .filter(|home| home.starts_with('/'))
        .ok_or_else(|| "the account home is not a directory".to_owned())?;
    Ok(format!("{home}/Library/Keychains/login.keychain-db"))
}

#[cfg(target_os = "macos")]
fn login_agent() -> Result<ureq::Agent, String> {
    let keychain = login_keychain()?;
    #[expect(
        clippy::disallowed_methods,
        reason = "the login keychain holds a root rustls-native-certs does not load"
    )]
    let output = std::process::Command::new("security")
        .args(["find-certificate", "-a", "-p", &keychain])
        .output()
        .map_err(|error| error.to_string())?;
    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).into_owned());
    }
    use ureq::rustls::pki_types::CertificateDer;
    use ureq::rustls::pki_types::pem::PemObject;
    let ders = CertificateDer::pem_slice_iter(&output.stdout)
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut roots = ureq::rustls::RootCertStore::empty();
    let (valid, _) = roots.add_parsable_certificates(ders);
    if valid == 0 {
        return Err("the login keychain has no certificates".to_owned());
    }
    let config = ureq::rustls::ClientConfig::builder_with_provider(
        ureq::rustls::crypto::ring::default_provider().into(),
    )
    .with_protocol_versions(&[&ureq::rustls::version::TLS12, &ureq::rustls::version::TLS13])
    .map_err(|error| error.to_string())?
    .with_root_certificates(roots)
    .with_no_client_auth();
    Ok(tarball::agent()
        .tls_config(std::sync::Arc::new(config))
        .build())
}

#[cfg(not(target_os = "macos"))]
fn login_agent() -> Result<ureq::Agent, String> {
    Err("no login keychain".to_owned())
}

fn fgj_config() -> String {
    let Ok(home) = std::env::var("HOME") else {
        return String::new();
    };
    std::fs::read_to_string(format!("{home}/.config/fgj/config.yaml")).unwrap_or_default()
}

fn token_in(text: &str) -> Option<String> {
    let mut host = false;
    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed == "git.example.invalid:" {
            host = true;
            continue;
        }
        if host && !line.starts_with("        ") && !trimmed.is_empty() {
            return None;
        }
        if host && let Some(value) = trimmed.strip_prefix("token:") {
            let value = value.trim().trim_matches('"');
            if !value.is_empty() {
                return Some(value.to_owned());
            }
        }
    }
    None
}

fn digest_hex(file: &[u8]) -> Result<String, String> {
    let text =
        std::str::from_utf8(file).map_err(|_| "the release digest is not text".to_owned())?;
    let hex = text
        .split_whitespace()
        .next()
        .ok_or("the release digest is empty")?;
    if hex.len() != 64 || !hex.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("the release digest is not a sha256".to_owned());
    }
    Ok(hex.to_ascii_lowercase())
}

fn verify_signature(
    home: &Path,
    archive: &[u8],
    signature: &[u8],
    signers: &str,
) -> Result<(), String> {
    let principal = signers
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty() && !line.starts_with('#'))
        .and_then(|line| line.split_whitespace().next())
        .ok_or("allowed_signers names no key")?;
    let root = home.join(".yi");
    std::fs::create_dir_all(&root).map_err(io(&root))?;
    let dir = root.join(format!(
        ".verify-{}-{}",
        std::process::id(),
        TICK.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_dir_all(&dir);
    private_dir(&dir)?;
    let checked = check_signature(&dir, archive, signature, signers, principal);
    let _ = std::fs::remove_dir_all(&dir);
    checked
}

/// Invariant: the signers file ssh-keygen trusts is writable by this user alone, so its dir
/// is made fresh and 0700 under `~/.yi`, never in a shared temp dir.
fn private_dir(dir: &Path) -> Result<(), String> {
    let mut builder = std::fs::DirBuilder::new();
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(io(dir))
}

#[expect(
    clippy::disallowed_methods,
    reason = "a release signature is checked with ssh-keygen -Y verify, the command scripts/sign.sh already runs"
)]
fn check_signature(
    dir: &Path,
    archive: &[u8],
    signature: &[u8],
    signers: &str,
    principal: &str,
) -> Result<(), String> {
    let signers_path = dir.join("allowed_signers");
    let signature_path = dir.join("archive.sig");
    std::fs::write(&signers_path, signers).map_err(io(&signers_path))?;
    std::fs::write(&signature_path, signature).map_err(io(&signature_path))?;
    let mut child = std::process::Command::new("ssh-keygen")
        .args(["-Y", "verify", "-f"])
        .arg(&signers_path)
        .args(["-I", principal, "-n", NAMESPACE, "-s"])
        .arg(&signature_path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|error| format!("ssh-keygen: {error}"))?;
    let mut stdin = child.stdin.take().ok_or("ssh-keygen took no archive")?;
    stdin
        .write_all(archive)
        .map_err(|error| error.to_string())?;
    drop(stdin);
    let finished = child
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    if finished.status.success() {
        Ok(())
    } else {
        Err("the release signature does not verify".to_owned())
    }
}

struct Unpacked {
    binary: Vec<u8>,
    skills: Vec<(String, Vec<u8>)>,
}

fn unpack(archive: &[u8]) -> Result<Unpacked, String> {
    let tar = tarball::gunzip(archive).map_err(|error| format!("the release archive {error}"))?;
    let mut binary = None;
    let mut skills = Vec::new();
    for entry in tarball::entries(&tar).map_err(|error| format!("the release archive: {error}"))? {
        match classify(&entry.name)? {
            Member::Binary if entry.regular => binary = Some(entry.bytes.to_vec()),
            Member::Skill(relative) if entry.regular => {
                skills.push((relative, entry.bytes.to_vec()))
            }
            Member::Binary => return Err("the release bin/yi is not a file".to_owned()),
            Member::Skill(_) | Member::Skip => {}
        }
    }
    Ok(Unpacked {
        binary: binary.ok_or("the release carries no bin/yi")?,
        skills,
    })
}

enum Member {
    Binary,
    Skill(String),
    Skip,
}

fn classify(path: &str) -> Result<Member, String> {
    if path.starts_with('/') || path.starts_with('\\') {
        return Err(format!("archive entry {path} is absolute"));
    }
    if path.split(['/', '\\']).any(|part| part == "..") {
        return Err(format!("archive entry {path} leaves the archive"));
    }
    let path = path.strip_prefix("./").unwrap_or(path);
    let inner = match path.split_once('/') {
        Some((head, rest)) if head.starts_with("yi-") => rest,
        _ => path,
    };
    if inner == "bin/yi" {
        return Ok(Member::Binary);
    }
    let Some(relative) = inner.strip_prefix("skills/") else {
        return Ok(Member::Skip);
    };
    if relative.is_empty() || relative.ends_with('/') {
        return Ok(Member::Skip);
    }
    if relative
        .split('/')
        .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(format!("archive entry {path} leaves the archive"));
    }
    Ok(Member::Skill(relative.to_owned()))
}

fn publish(
    dest: &Path,
    binary: &[u8],
    home: &Path,
    skills: &[(String, Vec<u8>)],
) -> Result<(), String> {
    let target = home.join(".yi/skills");
    let parent = dest
        .parent()
        .ok_or_else(|| format!("{} has no parent", dest.display()))?;
    std::fs::create_dir_all(parent).map_err(io(parent))?;
    let pid = std::process::id();
    let new_bin = parent.join(format!(".yi-new-{pid}"));
    let new_skills = home.join(format!(".yi/.skills-new-{pid}"));
    let result = write_exec(&new_bin, binary)
        .and_then(|()| stage_skills(&new_skills, &target, skills))
        .and_then(|()| swap(dest, &new_bin, &target, &new_skills));
    let _ = std::fs::remove_file(&new_bin);
    let _ = std::fs::remove_dir_all(&new_skills);
    result
}

fn swap(dest: &Path, new_bin: &Path, target: &Path, new_skills: &Path) -> Result<(), String> {
    let binary = tarball::swap_in(new_bin, dest)?;
    match tarball::swap_in(new_skills, target) {
        Ok(skills) => {
            skills.commit();
            binary.commit();
            Ok(())
        }
        Err(error) => Err(match binary.restore() {
            Ok(()) => error,
            Err(left) => format!("{error}; {left}"),
        }),
    }
}

fn write_exec(path: &Path, bytes: &[u8]) -> Result<(), String> {
    std::fs::write(path, bytes).map_err(io(path))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755)).map_err(io(path))?;
    }
    Ok(())
}

fn stage_skills(staging: &Path, target: &Path, skills: &[(String, Vec<u8>)]) -> Result<(), String> {
    let _ = std::fs::remove_dir_all(staging);
    std::fs::create_dir_all(staging).map_err(io(staging))?;
    if target.exists() {
        copy_tree(target, staging)?;
    }
    for (relative, bytes) in skills {
        let path = skill_file(staging, relative)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(io(parent))?;
        }
        std::fs::write(&path, bytes).map_err(io(&path))?;
    }
    Ok(())
}

fn copy_tree(from: &Path, to: &Path) -> Result<(), String> {
    let mut pending = vec![(from.to_path_buf(), to.to_path_buf())];
    while let Some((from, to)) = pending.pop() {
        for entry in std::fs::read_dir(&from).map_err(io(&from))? {
            let entry = entry.map_err(io(&from))?;
            let (source, copy) = (entry.path(), to.join(entry.file_name()));
            let kind = entry.file_type().map_err(io(&source))?;
            if kind.is_dir() {
                std::fs::create_dir(&copy).map_err(io(&copy))?;
                pending.push((source, copy));
            } else if kind.is_symlink() {
                let link = std::fs::read_link(&source).map_err(io(&source))?;
                #[cfg(unix)]
                std::os::unix::fs::symlink(link, &copy).map_err(io(&copy))?;
            } else {
                std::fs::copy(&source, &copy).map_err(io(&copy))?;
            }
        }
    }
    Ok(())
}

fn skill_file(root: &Path, relative: &str) -> Result<PathBuf, String> {
    let mut path = root.to_path_buf();
    for part in relative.split('/') {
        if part.is_empty() || part == "." || part == ".." {
            return Err(format!(
                "archive entry skills/{relative} leaves the archive"
            ));
        }
        path.push(part);
    }
    Ok(path)
}

fn write_receipt(home: &Path, receipt: &InstallReceipt) -> Result<(), String> {
    let dir = home.join(".yi");
    std::fs::create_dir_all(&dir).map_err(io(&dir))?;
    let dest = dir.join("install.json");
    let tmp = dir.join(format!(".install.json.{}", std::process::id()));
    let text = serde_json::to_string(receipt).map_err(|error| error.to_string())?;
    std::fs::write(&tmp, format!("{text}\n")).map_err(io(&tmp))?;
    std::fs::rename(&tmp, &dest).map_err(io(&dest))
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> String + '_ {
    move |error| format!("{}: {error}", path.display())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fgj_token_for_this_forge_is_read() {
        let text =
            "hosts:\n    git.example.invalid:\n        token: abc\n    other:\n        token: no\n";
        assert_eq!(token_in(text).as_deref(), Some("abc"));
        assert_eq!(token_in("hosts:\n").as_deref(), None);
    }

    #[test]
    fn the_token_goes_only_to_the_forge_host() {
        assert!(on_forge("https://git.example.invalid/api/v1/repos"));
        assert!(!on_forge("https://git.example.invalid.attacker.tld/x"));
        assert!(!on_forge("https://git.example.invalid@attacker.tld/x"));
        assert!(!on_forge("https://git.example.invalid"));
    }

    struct Tmp(PathBuf);

    impl Tmp {
        fn new() -> Result<Self, Box<dyn std::error::Error>> {
            let path = std::env::temp_dir().join(format!(
                "yi-update-test-{}-{}",
                std::process::id(),
                TICK.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&path)?;
            Ok(Self(path))
        }
    }

    impl Drop for Tmp {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn layout(tmp: &Path) -> Result<(PathBuf, PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let home = tmp.join("home");
        let prefix = tmp.join("prefix");
        std::fs::create_dir_all(home.join(".yi"))?;
        std::fs::create_dir_all(&prefix)?;
        let exe = prefix.join("yi");
        std::fs::write(&exe, b"old")?;
        Ok((home, prefix, exe))
    }

    fn tar_entry(out: &mut Vec<u8>, name: &str, data: &[u8]) -> Result<(), String> {
        if name.len() > 100 {
            return Err(format!("fixture name {name} exceeds the ustar field"));
        }
        let mut header = [0u8; 512];
        header
            .get_mut(..name.len())
            .ok_or("name")?
            .copy_from_slice(name.as_bytes());
        let size = format!("{:011o}\0", data.len());
        header
            .get_mut(124..136)
            .ok_or("size")?
            .copy_from_slice(size.as_bytes());
        header
            .get_mut(156..157)
            .ok_or("type")?
            .copy_from_slice(b"0");
        header
            .get_mut(257..262)
            .ok_or("magic")?
            .copy_from_slice(b"ustar");
        out.extend_from_slice(&header);
        out.extend_from_slice(data);
        out.resize(out.len().div_ceil(512) * 512, 0);
        Ok(())
    }

    fn gzip(tar: &[u8]) -> Vec<u8> {
        let mut out = vec![0x1f, 0x8b, 8, 0x08, 0, 0, 0, 0, 0, 3];
        out.extend_from_slice(b"yi.tar\0");
        out.extend(miniz_oxide::deflate::compress_to_vec(tar, 6));
        out.extend_from_slice(&[0; 8]);
        out
    }

    fn archive(binary: &[u8], skill: &[u8]) -> Result<Vec<u8>, String> {
        let mut tar = Vec::new();
        let root = "yi-0.2.0-aarch64-apple-darwin";
        tar_entry(&mut tar, &format!("{root}/bin/yi"), binary)?;
        tar_entry(&mut tar, &format!("{root}/skills/yi/SKILL.md"), skill)?;
        tar_entry(
            &mut tar,
            &format!("{root}/python/yi_runtime/keep.py"),
            b"python",
        )?;
        tar.extend_from_slice(&[0; 1024]);
        Ok(gzip(&tar))
    }

    struct Signed {
        digest: Vec<u8>,
        signature: Vec<u8>,
        signers: String,
    }

    #[expect(
        clippy::disallowed_methods,
        reason = "the fixture is signed with ssh-keygen, the same command a release uses"
    )]
    fn sign(dir: &Path, bytes: &[u8]) -> Result<Signed, Box<dyn std::error::Error>> {
        let key = dir.join("key");
        let made = std::process::Command::new("ssh-keygen")
            .args(["-q", "-t", "ed25519", "-N", "", "-C", "yi-test", "-f"])
            .arg(&key)
            .status()?;
        if !made.success() {
            return Err("ssh-keygen could not make a key".into());
        }
        let file = dir.join("release.tar.gz");
        std::fs::write(&file, bytes)?;
        let signed = std::process::Command::new("ssh-keygen")
            .args(["-Y", "sign", "-n", "yi-release", "-f"])
            .arg(&key)
            .arg(&file)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()?;
        if !signed.success() {
            return Err("ssh-keygen could not sign the fixture".into());
        }
        let public = std::fs::read_to_string(dir.join("key.pub"))?;
        let signers = format!("yi-test {}\n", public.trim());
        Ok(Signed {
            digest: format!("{}  release.tar.gz\n", tarball::sha256_hex(bytes)).into_bytes(),
            signature: std::fs::read(format!("{}.sig", file.display()))?,
            signers,
        })
    }

    fn run_job(
        home: &Path,
        prefix: &Path,
        exe: &Path,
        current: &str,
        signers: &str,
        version: &str,
        files: &[(&str, &[u8])],
    ) -> Result<String, String> {
        let name = format!("yi-{version}-aarch64-apple-darwin.tar.gz");
        let latest = serde_json::json!({
            "tag_name": format!("v{version}"),
            "assets": [
                {"name": name, "browser_download_url": "archive"},
                {"name": format!("{name}.sha256"), "browser_download_url": "digest"},
                {"name": format!("{name}.sig"), "browser_download_url": "signature"},
            ],
        })
        .to_string();
        let latest_url = format!("{FORGE}/api/v1/repos/apex/yi/releases/latest");
        apply(Apply {
            home,
            prefix,
            exe,
            current,
            target: "aarch64-apple-darwin",
            signers,
            get: &|url: &str| {
                if url == latest_url {
                    return Ok(latest.clone().into_bytes());
                }
                files
                    .iter()
                    .find(|(name, _)| *name == url)
                    .map(|(_, bytes)| bytes.to_vec())
                    .ok_or_else(|| format!("downloaded {url}"))
            },
        })
    }

    #[test]
    fn a_binary_outside_the_prefix_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let home = tmp.0.join("home");
        let prefix = tmp.0.join("prefix");
        let other = tmp.0.join("other");
        std::fs::create_dir_all(&home)?;
        std::fs::create_dir_all(&prefix)?;
        std::fs::create_dir_all(&other)?;
        let exe = other.join("yi");
        std::fs::write(&exe, b"stay")?;
        let error =
            run_job(&home, &prefix, &exe, "0.1.0", "unused\n", "0.2.0", &[]).expect_err("refused");
        assert!(error.contains(&exe.display().to_string()), "{error}");
        assert_eq!(std::fs::read(&exe)?, b"stay");
        Ok(())
    }

    #[test]
    fn a_bad_digest_leaves_the_binary() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let (home, prefix, exe) = layout(&tmp.0)?;
        let packed = archive(b"new", b"skill")?;
        let error = run_job(
            &home,
            &prefix,
            &exe,
            "0.1.0",
            "unused\n",
            "0.2.0",
            &[
                ("archive", &packed),
                (
                    "digest",
                    b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa  x\n",
                ),
            ],
        )
        .expect_err("digest");
        assert_eq!(error, "the release digest does not match");
        assert_eq!(std::fs::read(&exe)?, b"old");
        Ok(())
    }

    #[test]
    fn a_bad_signature_leaves_the_binary() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let (home, prefix, exe) = layout(&tmp.0)?;
        let packed = archive(b"new", b"skill")?;
        let Signed {
            digest,
            mut signature,
            signers,
        } = sign(&tmp.0, &packed)?;
        let mid = signature.len() / 2;
        let byte = signature.get_mut(mid).ok_or("empty signature")?;
        *byte ^= 0x01;
        let error = run_job(
            &home,
            &prefix,
            &exe,
            "0.1.0",
            &signers,
            "0.2.0",
            &[
                ("archive", packed.as_slice()),
                ("digest", digest.as_slice()),
                ("signature", signature.as_slice()),
            ],
        )
        .expect_err("signature");
        assert_eq!(error, "the release signature does not verify");
        assert_eq!(std::fs::read(&exe)?, b"old");
        Ok(())
    }

    #[test]
    fn a_signed_release_replaces_the_prefix_binary_and_skills()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let (home, prefix, exe) = layout(&tmp.0)?;
        std::fs::create_dir_all(home.join(".yi/skills/yi"))?;
        std::fs::write(home.join(".yi/skills/yi/SKILL.md"), b"stale")?;
        std::fs::write(home.join(".yi/skills/yi/mine.md"), b"mine")?;
        let prior = serde_json::json!({
            "prefix": prefix.display().to_string(),
            "version": "0.1.0",
            "target": "aarch64-apple-darwin",
            "note": "keep"
        });
        std::fs::write(home.join(".yi/install.json"), format!("{prior}\n"))?;
        let packed = archive(b"new", b"skill body")?;
        let Signed {
            digest,
            signature,
            signers,
        } = sign(&tmp.0, &packed)?;
        let line = run_job(
            &home,
            &prefix,
            &exe,
            "0.1.0",
            &signers,
            "0.2.0",
            &[
                ("archive", packed.as_slice()),
                ("digest", digest.as_slice()),
                ("signature", signature.as_slice()),
            ],
        )?;
        assert_eq!(
            line,
            format!("installed 0.1.0 → 0.2.0 at {}", prefix.display())
        );
        assert_eq!(std::fs::read(&exe)?, b"new");
        assert_eq!(
            std::fs::read(home.join(".yi/skills/yi/SKILL.md"))?,
            b"skill body"
        );
        assert_eq!(std::fs::read(home.join(".yi/skills/yi/mine.md"))?, b"mine");
        assert!(!home.join(".yi/python").exists());
        let saved: InstallReceipt =
            serde_json::from_slice(&std::fs::read(home.join(".yi/install.json"))?)?;
        assert_eq!(saved.version, "0.2.0");
        assert_eq!(saved.target, "aarch64-apple-darwin");
        assert_eq!(saved.prefix, prefix.display().to_string());
        assert_eq!(
            saved.extra.get("note").and_then(|value| value.as_str()),
            Some("keep")
        );
        Ok(())
    }

    #[test]
    fn the_current_version_downloads_nothing() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let (home, prefix, exe) = layout(&tmp.0)?;
        let line = run_job(&home, &prefix, &exe, "0.2.0", "unused\n", "0.2.0", &[])?;
        assert_eq!(line, "already 0.2.0");
        assert_eq!(std::fs::read(&exe)?, b"old");
        Ok(())
    }

    #[test]
    fn an_entry_that_leaves_the_archive_is_refused() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let (home, prefix, exe) = layout(&tmp.0)?;
        let mut tar = Vec::new();
        tar_entry(
            &mut tar,
            "yi-0.2.0-aarch64-apple-darwin/../../escape",
            b"never",
        )?;
        tar_entry(&mut tar, "yi-0.2.0-aarch64-apple-darwin/bin/yi", b"new")?;
        tar.extend_from_slice(&[0; 1024]);
        let packed = gzip(&tar);
        let Signed {
            digest,
            signature,
            signers,
        } = sign(&tmp.0, &packed)?;
        let error = run_job(
            &home,
            &prefix,
            &exe,
            "0.1.0",
            &signers,
            "0.2.0",
            &[
                ("archive", packed.as_slice()),
                ("digest", digest.as_slice()),
                ("signature", signature.as_slice()),
            ],
        )
        .expect_err("escape");
        assert!(error.contains("leaves the archive"), "{error}");
        assert_eq!(std::fs::read(&exe)?, b"old");
        assert!(!tmp.0.join("escape").exists());
        Ok(())
    }

    #[test]
    fn a_failed_binary_rename_leaves_the_skills() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let home = tmp.0.join("home");
        std::fs::create_dir_all(home.join(".yi/skills/yi"))?;
        std::fs::write(home.join(".yi/skills/yi/SKILL.md"), b"old skill")?;
        std::fs::write(home.join(".yi/skills/yi/mine.md"), b"mine")?;
        let dest = tmp.0.join("prefix/yi");
        std::fs::create_dir_all(tmp.0.join("prefix"))?;
        std::fs::write(&dest, b"old")?;
        let held = format!("yi.old-{}", std::process::id());
        std::fs::write(tmp.0.join("prefix").join(&held), b"held")?;
        let skills = [("yi/SKILL.md".to_owned(), b"new skill".to_vec())];
        let error = publish(&dest, b"new", &home, &skills).expect_err("rename");
        assert_eq!(std::fs::read(&dest)?, b"old");
        assert!(error.contains(&dest.display().to_string()), "{error}");
        assert_eq!(
            std::fs::read(home.join(".yi/skills/yi/SKILL.md"))?,
            b"old skill"
        );
        assert_eq!(std::fs::read(home.join(".yi/skills/yi/mine.md"))?, b"mine");
        let mut left: Vec<_> = std::fs::read_dir(home.join(".yi"))?
            .map(|entry| entry.map(|entry| entry.file_name()))
            .collect::<Result<_, _>>()?;
        left.extend(
            std::fs::read_dir(tmp.0.join("prefix"))?
                .map(|entry| entry.map(|entry| entry.file_name()))
                .collect::<Result<Vec<_>, _>>()?,
        );
        left.sort();
        assert_eq!(left, ["skills", "yi", &held], "no staging left behind");
        Ok(())
    }

    fn old_install(tmp: &Path) -> Result<(PathBuf, PathBuf), Box<dyn std::error::Error>> {
        let home = tmp.join("home");
        std::fs::create_dir_all(home.join(".yi/skills/yi/SKILL.md"))?;
        std::fs::write(home.join(".yi/skills/yi/SKILL.md/mine.md"), b"mine")?;
        let dest = tmp.join("prefix/yi");
        std::fs::create_dir_all(tmp.join("prefix"))?;
        std::fs::write(&dest, b"old")?;
        Ok((home, dest))
    }

    #[test]
    fn a_failure_mid_merge_leaves_the_old_binary_and_skills()
    -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let (home, dest) = old_install(&tmp.0)?;
        let skills = [
            ("a/one.md".to_owned(), b"one".to_vec()),
            ("yi/SKILL.md".to_owned(), b"new skill".to_vec()),
        ];
        publish(&dest, b"new", &home, &skills).expect_err("collision");
        assert_eq!(std::fs::read(&dest)?, b"old");
        assert!(!home.join(".yi/skills/a").exists());
        assert_eq!(
            std::fs::read(home.join(".yi/skills/yi/SKILL.md/mine.md"))?,
            b"mine"
        );
        Ok(())
    }

    #[test]
    fn a_failed_skills_swap_restores_the_old_binary() -> Result<(), Box<dyn std::error::Error>> {
        let tmp = Tmp::new()?;
        let (home, dest) = old_install(&tmp.0)?;
        let blocker = home.join(format!(".yi/skills.old-{}", std::process::id()));
        std::fs::create_dir_all(&blocker)?;
        std::fs::write(blocker.join("held"), b"held")?;
        let skills = [("a/one.md".to_owned(), b"one".to_vec())];
        publish(&dest, b"new", &home, &skills).expect_err("swap");
        assert_eq!(std::fs::read(&dest)?, b"old");
        assert!(!home.join(".yi/skills/a").exists());
        assert_eq!(
            std::fs::read(home.join(".yi/skills/yi/SKILL.md/mine.md"))?,
            b"mine"
        );
        Ok(())
    }
}
