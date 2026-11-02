//! A tree plugin is a zip published whole: the console's browser runs the pinned Chrome for
//! Testing headless shell from it, beside the libraries and resources it loads.
use std::path::PathBuf;

use yi_runtime::plugin::{Plugin, Shape, State, add, state};
use yi_tools::plugin::Asset;

use crate::scratch::Scratch;

type TestResult = Result<(), Box<dyn std::error::Error>>;

/// Info-ZIP 3.0's `zip -r -D` of a program, a library beside it and a resource: the producer
/// and layout of a Chrome for Testing archive, at a few hundred bytes.
const ZIP: &[u8] = include_bytes!("fixtures/plugin/tool-2.0.zip");
const PROGRAM: &[u8] = b"#!/bin/sh\necho tool\n";

fn tree(archive: &[u8], member_sha256: &str) -> Plugin {
    let asset = Asset {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        url: "https://example.invalid/tool-2.0.zip",
        sha256: Box::leak(yi_kernel::tarball::sha256_hex(archive).into_boxed_str()),
        member_sha256: Box::leak(member_sha256.to_owned().into_boxed_str()),
        member: "tool-2.0/tool",
    };
    Plugin {
        name: "tool",
        version: "2.0",
        summary: "a test tree",
        shape: Shape::Tree,
        card: "",
        skills: &[],
        assets: Box::leak(Box::new([asset])),
    }
}

fn program_sha() -> String {
    yi_kernel::tarball::sha256_hex(PROGRAM)
}

#[test]
fn a_zip_tree_is_published_whole_and_a_bad_part_publishes_nothing() -> TestResult {
    let scratch = Scratch::new("plugin-tree")?;
    let home = scratch.home()?;
    let tool = tree(ZIP, &program_sha());
    let path = add(&tool, &home, |_| Ok(ZIP.to_vec()))?;
    assert_eq!(path, tool.dir(&home).join("tool-2.0/tool"));
    assert_eq!(std::fs::read(&path)?, PROGRAM);
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path)?.permissions().mode() & 0o777,
            0o755
        );
        let lib = tool.dir(&home).join("tool-2.0/lib/libtool.so");
        assert_eq!(std::fs::metadata(&lib)?.permissions().mode() & 0o777, 0o644);
        let text = std::fs::read_to_string(&lib)?;
        assert_eq!(
            text.lines().count(),
            8,
            "the deflated library unpacks whole"
        );
    }
    assert!(tool.dir(&home).join("tool-2.0/resources.pak").is_file());
    assert!(matches!(state(&tool, &home), State::Installed(p) if p == path));
    let chrome = Plugin::named("chrome").ok_or("no chrome row")?;
    if let Some(program) = chrome.program(&home) {
        std::fs::create_dir_all(program.parent().ok_or("no parent")?)?;
        std::fs::write(program, PROGRAM)?;
        assert_eq!(
            yi_tools::plugin::search_path(&home),
            None,
            "a tree is not a program the model runs by name"
        );
    }
    std::fs::write(&path, b"#!/bin/sh\necho poisoned\n")?;
    let error = add(&tool, &home, |_| Err("no download is needed".to_owned()))
        .err()
        .ok_or("a poisoned tree was adopted")?;
    assert!(error.contains("yi plugin remove tool"), "{error}");
    Ok(())
}

#[test]
fn a_tampered_zip_an_escaping_entry_or_a_wrong_member_installs_nothing() -> TestResult {
    let scratch = Scratch::new("plugin-tree-refused")?;
    let home = scratch.home()?;
    let mut tampered = ZIP.to_vec();
    if let Some(byte) = tampered.get_mut(ZIP.len() / 2) {
        *byte ^= 1;
    }
    let error = add(&tree(ZIP, &program_sha()), &home, |_| Ok(tampered.clone()))
        .err()
        .ok_or("a tampered archive installed")?;
    assert!(error.contains("not installing"), "{error}");
    let escaping = rename_entry(ZIP, b"tool-2.0/resources.pak", b"tool-2.0/../../escapes");
    let error = add(&tree(&escaping, &program_sha()), &home, |_| {
        Ok(escaping.clone())
    })
    .err()
    .ok_or("an escaping entry installed")?;
    assert!(
        error.contains("tool-2.0/../../escapes leaves it"),
        "{error}"
    );
    let wrong = "0".repeat(64);
    let error = add(&tree(ZIP, &wrong), &home, |_| Ok(ZIP.to_vec()))
        .err()
        .ok_or("a member off its pin installed")?;
    assert!(error.contains(&format!("yi pins {wrong}")), "{error}");
    assert!(!home.join(".yi/plugins/tool/2.0").exists());
    assert!(!home.join(".yi/escapes").exists() && !home.join(".yi/plugins/escapes").exists());
    assert_eq!(
        leftovers(&home)?,
        Vec::<String>::new(),
        "no staging is left behind"
    );
    Ok(())
}

#[test]
fn eight_threads_adding_one_new_tree_end_with_one_valid_install() -> TestResult {
    let scratch = Scratch::new("plugin-tree-race")?;
    let home = scratch.home()?;
    let tool = tree(ZIP, &program_sha());
    let results: Vec<Result<PathBuf, String>> = std::thread::scope(|scope| {
        let handles: Vec<_> = (0..8)
            .map(|_| scope.spawn(|| add(&tool, &home, |_| Ok(ZIP.to_vec()))))
            .collect();
        handles
            .into_iter()
            .map(|handle| handle.join().unwrap_or_else(|_| Err("panicked".to_owned())))
            .collect()
    });
    for result in results {
        assert_eq!(result?, tool.dir(&home).join("tool-2.0/tool"));
    }
    assert_eq!(
        std::fs::read(tool.dir(&home).join("tool-2.0/tool"))?,
        PROGRAM
    );
    assert_eq!(leftovers(&home)?, ["2.0"]);
    Ok(())
}

/// The same entry under another name of the same length, in its local and central headers.
fn rename_entry(zip: &[u8], from: &[u8], to: &[u8]) -> Vec<u8> {
    let mut out = zip.to_vec();
    let mut at = 0;
    while let Some(found) = out
        .get(at..)
        .and_then(|rest| rest.windows(from.len()).position(|w| w == from))
    {
        let start = at + found;
        if let Some(name) = out.get_mut(start..start + to.len()) {
            name.copy_from_slice(to);
        }
        at = start + to.len();
    }
    out
}

fn leftovers(home: &std::path::Path) -> std::io::Result<Vec<String>> {
    let Ok(entries) = std::fs::read_dir(home.join(".yi/plugins/tool")) else {
        return Ok(Vec::new());
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.map(|entry| entry.file_name().to_string_lossy().into_owned()))
        .collect::<Result<_, _>>()?;
    names.sort();
    Ok(names)
}

/// A version bump that misses one target's URL would leave that target fetching a build whose
/// digest no longer matches, found only on that platform.
#[test]
fn every_chrome_asset_names_the_pinned_version_s_headless_shell() -> TestResult {
    let chrome = Plugin::named("chrome").ok_or("no chrome row")?;
    assert_eq!(chrome.shape, Shape::Tree);
    let mut targets = Vec::new();
    for asset in chrome.assets {
        let platform = match (asset.os, asset.arch) {
            ("macos", "aarch64") => "mac-arm64",
            ("macos", "x86_64") => "mac-x64",
            ("linux", "x86_64") => "linux64",
            other => return Err(format!("Chrome for Testing has no build for {other:?}").into()),
        };
        targets.push(platform);
        let url = format!(
            "https://storage.googleapis.com/chrome-for-testing-public/{}/{platform}/chrome-headless-shell-{platform}.zip",
            chrome.version
        );
        assert_eq!(asset.url, url);
        assert_eq!(
            asset.member,
            format!("chrome-headless-shell-{platform}/chrome-headless-shell")
        );
        for digest in [asset.sha256, asset.member_sha256] {
            let hex = digest
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
            assert!(digest.len() == 64 && hex, "{platform}: {digest}");
        }
    }
    targets.sort_unstable();
    assert_eq!(targets, ["linux64", "mac-arm64", "mac-x64"]);
    Ok(())
}

/// A host plugin is the program the model's bash runs by name, so an installed one must reach the
/// child's PATH; a row on disk that nothing puts on PATH is a plugin the model cannot call.
#[test]
fn an_installed_host_plugin_is_on_the_child_path() -> TestResult {
    let scratch = Scratch::new("plugin-host")?;
    let home = scratch.home()?;
    let desktop = Plugin::named("agent-desktop").ok_or("no agent-desktop row")?;
    assert_eq!(desktop.shape, Shape::Host);
    assert_eq!(
        yi_tools::plugin::search_path(&home),
        None,
        "a fresh home has no program on PATH"
    );
    let program = desktop.program(&home).ok_or("no program path")?;
    std::fs::create_dir_all(program.parent().ok_or("no parent")?)?;
    std::fs::write(&program, PROGRAM)?;
    let path = yi_tools::plugin::search_path(&home).ok_or("a host plugin is not on PATH")?;
    let first = std::env::split_paths(&path)
        .next()
        .ok_or("no first entry")?;
    assert_eq!(first, desktop.dir(&home));
    Ok(())
}

/// Upstream publishes a bare program, so its digest is the download's and a wrong one installs nothing.
#[test]
fn a_bare_program_is_published_and_a_tampered_one_is_not() -> TestResult {
    let scratch = Scratch::new("plugin-bare")?;
    let home = scratch.home()?;
    let digest: &'static str = Box::leak(program_sha().into_boxed_str());
    let asset = Asset {
        os: std::env::consts::OS,
        arch: std::env::consts::ARCH,
        url: "https://example.invalid/tool",
        sha256: digest,
        member_sha256: digest,
        member: "",
    };
    let tool = Plugin {
        name: "tool",
        version: "2.0",
        summary: "a bare test program",
        shape: Shape::Host,
        card: "",
        skills: &[],
        assets: Box::leak(Box::new([asset])),
    };
    assert!(add(&tool, &home, |_| Ok(b"tampered".to_vec())).is_err());
    assert!(tool.installed(&home).is_none());
    let path = add(&tool, &home, |_| Ok(PROGRAM.to_vec()))?;
    assert_eq!(path, tool.dir(&home).join("tool"));
    assert_eq!(std::fs::read(&path)?, PROGRAM);
    let mode = std::os::unix::fs::PermissionsExt::mode(&std::fs::metadata(&path)?.permissions());
    assert_eq!(mode & 0o777, 0o755);
    Ok(())
}

/// A daemon missing these reads the repo's `agent-browser.json` and starts a browser with no box.
#[test]
fn an_installed_browser_starts_with_its_own_config_and_socket() -> TestResult {
    let scratch = Scratch::new("plugin-browser")?;
    let home = scratch.home()?;
    let browser = Plugin::named(yi_tools::plugin::BROWSER).ok_or("no agent-browser row")?;
    assert_eq!(browser.shape, Shape::Host, "--all must never install it");
    assert!(yi_tools::plugin::browser_env(&home).is_empty());
    let program = browser.program(&home).ok_or("no program path")?;
    std::fs::create_dir_all(program.parent().ok_or("no parent")?)?;
    std::fs::write(&program, PROGRAM)?;
    let env = yi_tools::plugin::browser_env(&home);
    let value = |name: &str| {
        env.iter()
            .find(|(key, _)| *key == name)
            .map(|(_, v)| v.clone())
    };
    assert_eq!(
        value("AGENT_BROWSER_CONFIG"),
        Some(browser.dir(&home).join("config.json").into_os_string())
    );
    assert!(value("AGENT_BROWSER_SOCKET_DIR").is_some());
    assert!(value("AGENT_BROWSER_IDLE_TIMEOUT_MS").is_some());
    Ok(())
}
