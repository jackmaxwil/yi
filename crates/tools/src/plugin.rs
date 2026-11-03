use std::ffi::OsString;
use std::path::{Path, PathBuf};

pub struct Asset {
    pub os: &'static str,
    pub arch: &'static str,
    pub url: &'static str,
    pub sha256: &'static str,
    /// Invariant: digest of the member itself, so a file already at the pinned path is
    /// re-verified before yi adopts it as the pin.
    pub member_sha256: &'static str,
    /// Empty when the download is the program itself, with no archive around it.
    pub member: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shape {
    /// A tar.gz whose `member` is published as the plugin's name, with the skills its row
    /// names: a program the model runs, first on a spawn's PATH, its card in every session.
    Program,
    /// A zip published whole, `member` naming the executable inside it: a program Yi starts
    /// itself, so it is off PATH and the model gets no card.
    Tree,
    /// A [`Shape::Program`] that drives this machine rather than a repository: only an explicit
    /// `yi plugin add` installs it, so `--all` and the eval containers never carry it.
    Host,
}

impl Shape {
    /// Whether the model runs it by name: first on a spawn's PATH, its card in every session.
    #[must_use]
    pub fn on_path(self) -> bool {
        matches!(self, Self::Program | Self::Host)
    }
}

/// A program `yi plugin add` installs on request; the code that reads it is compiled into yi.
pub struct Plugin {
    pub name: &'static str,
    pub version: &'static str,
    pub summary: &'static str,
    pub shape: Shape,
    pub card: &'static str,
    /// Skill directories in the release, published beside the program and read on demand.
    pub skills: &'static [&'static str],
    pub assets: &'static [Asset],
}

/// Bump a version with every digest beside it, from the release's own `.sha256` files, and
/// record its skills' `sha256sum` as `tests/fixtures/ripwire/skills-<version>.sha256`.
pub const PLUGINS: [Plugin; 4] = [
    Plugin {
        name: "ripwire",
        version: "0.6.5",
        summary: "a code map: an edit's broken callers, a symbol's callers, the code a task touches",
        shape: Shape::Program,
        card: include_str!("prompts/ripwire.md"),
        // Every skill the release ships but `ripwire-mcp`, which drives an MCP server Yi does not run.
        skills: &[
            "ripwire-before-you-build",
            "ripwire-change-check",
            "ripwire-find-bug",
            "ripwire-fresh-eyes",
            "ripwire-graph-query",
            "ripwire-handoff",
            "ripwire-layers",
            "ripwire-navigate",
            "ripwire-opt-remarks",
            "ripwire-orient",
            "ripwire-perf-target",
            "ripwire-quality-bar",
            "ripwire-reuse-first",
            "ripwire-router",
            "ripwire-security-scan",
            "ripwire-write-tests",
        ],
        assets: &[
            Asset {
                os: "macos",
                arch: "aarch64",
                url: "https://github.com/redhat-et/ripwire/releases/download/v0.6.5/ripwire-0.6.5-macos-arm64.tar.gz",
                sha256: "17a68cfba3f0d6e1390686829239a3753b454aae9fe4ecaf52e0f365d890afc0",
                member_sha256: "8fc6abdeb9f0a597412431947292e9b5f3c6392403ce4de4afbae24b0173f64d",
                member: "ripwire-0.6.5-macos-arm64/ripwire",
            },
            Asset {
                os: "linux",
                arch: "aarch64",
                url: "https://github.com/redhat-et/ripwire/releases/download/v0.6.5/ripwire-0.6.5-linux-arm64.tar.gz",
                sha256: "25e5f37e2985830bbff189879797988b862e36521413a6e3654630a0e1f0b108",
                member_sha256: "ad3291cf0854cdd5be8661b90be803b1caf75ae24e061950f142158df2755f8d",
                member: "ripwire-0.6.5-linux-arm64/ripwire",
            },
            Asset {
                os: "linux",
                arch: "x86_64",
                url: "https://github.com/redhat-et/ripwire/releases/download/v0.6.5/ripwire-0.6.5-linux-x64.tar.gz",
                sha256: "5c5794612f5f06632ada7c27a0f5c8f400748a70f707b64ec4d238c4fba2f7ea",
                member_sha256: "30f961efca7c7bff70cec72b84524b03d65a93097724ccd7ee1635801f621284",
                member: "ripwire-0.6.5-linux-x64/ripwire",
            },
        ],
    },
    // Chrome for Testing's headless shell; each digest was measured on the downloaded archive
    // and on its member, since the release publishes no sha256 of either.
    Plugin {
        name: "chrome",
        version: "155.0.8059.39",
        summary: "Chrome for Testing's headless shell (~100 MB), which the console's browser pane \
                  runs when no Chrome or Chromium is installed",
        shape: Shape::Tree,
        card: "",
        skills: &[],
        assets: &[
            Asset {
                os: "macos",
                arch: "aarch64",
                url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.39/mac-arm64/chrome-headless-shell-mac-arm64.zip",
                sha256: "b3e093c06001c41e68decbc8dd4a62f9efe4bf4e4dd247a686ea531863448d75",
                member_sha256: "93039c9fbd18302515c40f1d9d147967402ac67aa0e81b44c623f889315d9285",
                member: "chrome-headless-shell-mac-arm64/chrome-headless-shell",
            },
            Asset {
                os: "macos",
                arch: "x86_64",
                url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.39/mac-x64/chrome-headless-shell-mac-x64.zip",
                sha256: "6338a784c691f42dd1ed6aeeef650171c7d72cf74bdb8450d0079864e72a8074",
                member_sha256: "9e4becd4215f4ba236b87a0c6168fcddf5d1516006af6250b52a5f1ecec22e82",
                member: "chrome-headless-shell-mac-x64/chrome-headless-shell",
            },
            Asset {
                os: "linux",
                arch: "x86_64",
                url: "https://storage.googleapis.com/chrome-for-testing-public/155.0.8059.39/linux64/chrome-headless-shell-linux64.zip",
                sha256: "39dcb8c46550632a3d911850ab3b8af840b4e3f6d8622faa2018eb8756278786",
                member_sha256: "d8f762c0153b944bc0cd747fa6ae2e0497d29d703049833e2942e15fc646b82b",
                member: "chrome-headless-shell-linux64/chrome-headless-shell",
            },
        ],
    },
    // Only the main binary is published: the tarball's second file serves the clipboard alone,
    // and snapshot, click and close-app were run without it.
    Plugin {
        name: "agent-desktop",
        version: "1.0.2",
        summary: "operates any Mac app through its accessibility tree, asking once per app",
        shape: Shape::Host,
        card: include_str!("prompts/agent_desktop.md"),
        skills: &[],
        assets: &[Asset {
            os: "macos",
            arch: "aarch64",
            url: "https://github.com/vercel-labs/agent-desktop/releases/download/v1.0.2/agent-desktop-v1.0.2-aarch64-apple-darwin.tar.gz",
            sha256: "7bee385196d7b478de80cf05ca4304aaa38f0eba593b3406ad3c247c1773be19",
            member_sha256: "bf0a3c9256512f2f57288d37ccd17d7f03665fccc53c2f74011a49b98e61ba59",
            member: "agent-desktop",
        }],
    },
    Plugin {
        name: BROWSER,
        version: yi_permission::BROWSER_VERSION,
        summary: "drives a headless Chrome held in a sandbox: this Mac's apps unasked, the \
                  internet after one yes per session",
        shape: Shape::Host,
        card: include_str!("prompts/agent_browser.md"),
        skills: &[],
        assets: &[Asset {
            os: "macos",
            arch: "aarch64",
            url: "https://github.com/vercel-labs/agent-browser/releases/download/v0.41.0/agent-browser-darwin-arm64",
            sha256: "a402fef2489c221b17522aa21f103ca5dc47f00ff5879d4a72c39881544730ab",
            member_sha256: "a402fef2489c221b17522aa21f103ca5dc47f00ff5879d4a72c39881544730ab",
            member: "",
        }],
    },
];

pub const BROWSER: &str = "agent-browser";

/// Incident: the daemon idles an hour by default, and while a web session's Chrome lives any
/// contained program can reach it on loopback, so it goes as soon as the model stops using it.
const BROWSER_IDLE_MS: &str = "300000";

/// The file a host daemon runs as Chrome, under the plugin's directory since a contained command
/// could rewrite one in tmp, and tmp is where the daemon would run it from.
fn browser_config(home: &Path) -> Option<PathBuf> {
    let plugin = Plugin::named(BROWSER)?;
    plugin.installed(home)?;
    Some(plugin.dir(home).join("config.json"))
}

/// What the boxed daemon starts with. Invariant: its config names the file, so a repo's
/// `agent-browser.json` is never read, and a socket of its own is no daemon the user started.
pub fn browser_env(home: &Path) -> Vec<(&'static str, OsString)> {
    let Some(config) = browser_config(home).filter(|_| yi_permission::browser_boxed(home)) else {
        return Vec::new();
    };
    let sockets = config.with_file_name("s");
    vec![
        ("AGENT_BROWSER_CONFIG", config.into_os_string()),
        ("AGENT_BROWSER_SOCKET_DIR", sockets.into_os_string()),
        ("AGENT_BROWSER_IDLE_TIMEOUT_MS", BROWSER_IDLE_MS.into()),
    ]
}

/// Writes the config and the script the daemon runs as Chrome: the same box either way, with the
/// internet only for the `web` session. Rewritten by every add, since `sync` brings a new store list.
pub fn write_browser_box(home: &Path, chrome: &Path) -> Result<(), String> {
    let config = browser_config(home).ok_or("agent-browser is not installed")?;
    let script = config.with_file_name("chrome-box");
    let sandbox = crate::sandbox::Sandbox::for_browser(home);
    let line = |web: bool| -> String {
        let (seatbelt, args) = sandbox.wrap_browser(web, home, &chrome.to_string_lossy());
        let args = args.iter().map(|arg| crate::process::quoted(arg));
        let words: Vec<String> = std::iter::once(seatbelt).chain(args).collect();
        format!("exec {} \"$@\"", words.join(" "))
    };
    let text = format!(
        "#!/bin/sh\ncase \"$AGENT_BROWSER_SESSION\" in\n{web}) {open} ;;\n*) {closed} ;;\nesac\n",
        web = yi_permission::BROWSER_WEB,
        open = line(true),
        closed = line(false),
    );
    let path = script.to_string_lossy();
    let json = serde_json::json!({ "executablePath": path }).to_string();
    let replace = |to: &Path, bytes: &str, mode: u32| -> std::io::Result<()> {
        use std::os::unix::fs::PermissionsExt;
        let staged = to.with_extension("staged");
        std::fs::write(&staged, bytes)?;
        std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(mode))?;
        std::fs::rename(&staged, to)
    };
    replace(&script, &text, 0o755)
        .and_then(|()| replace(&config, &json, 0o644))
        .map_err(|error| format!("{}: {error}", script.display()))
}

impl Plugin {
    pub fn named(name: &str) -> Option<&'static Self> {
        PLUGINS.iter().find(|plugin| plugin.name == name)
    }

    pub fn asset(&self) -> Option<&'static Asset> {
        let (os, arch) = (std::env::consts::OS, std::env::consts::ARCH);
        self.assets
            .iter()
            .find(|asset| asset.os == os && asset.arch == arch)
    }

    pub fn root(&self, home: &Path) -> PathBuf {
        home.join(".yi").join("plugins").join(self.name)
    }

    /// Invariant: only `yi plugin add` writes here, by one rename after the digest matched.
    pub fn dir(&self, home: &Path) -> PathBuf {
        self.root(home).join(self.version)
    }

    /// Where the pinned version's program sits once installed.
    pub fn program(&self, home: &Path) -> Option<PathBuf> {
        match self.shape {
            Shape::Program | Shape::Host => Some(self.dir(home).join(self.name)),
            Shape::Tree => self.asset().map(|asset| self.dir(home).join(asset.member)),
        }
    }

    /// The program at the version this yi pins; another version on disk is not it.
    pub fn installed(&self, home: &Path) -> Option<PathBuf> {
        self.program(home).filter(|path| path.is_file())
    }

    /// The installed skill directories; a copy found only on PATH brings none Yi can vouch for.
    pub fn skill_root(&self, home: &Path) -> Option<PathBuf> {
        self.installed(home)
            .map(|_| self.dir(home).join("skills"))
            .filter(|root| root.is_dir())
    }

    /// A tree's name on PATH is someone else's program: `chrome` there is not the headless shell.
    pub fn found(&self, home: &Path) -> Option<PathBuf> {
        self.installed(home).or_else(|| match self.shape {
            Shape::Program | Shape::Host => crate::syntax::on_path(self.name),
            Shape::Tree => None,
        })
    }
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// Every installed program plugin's directory, in the order a PATH leads with them.
pub fn installed_dirs(home: &Path) -> Vec<PathBuf> {
    PLUGINS
        .iter()
        .filter(|plugin| plugin.shape.on_path() && plugin.installed(home).is_some())
        .map(|plugin| plugin.dir(home))
        .collect()
}

/// PATH with every installed program plugin's directory first, so a spawn and the model's bash
/// both find it by name; None when none is installed, which leaves a child's PATH untouched.
pub fn search_path(home: &Path) -> Option<OsString> {
    let dirs = installed_dirs(home);
    if dirs.is_empty() {
        return None;
    }
    // Invariant: a child with no PATH searched execvp's default, /usr/bin:/bin; a PATH of the
    // plugin dirs alone would lose both, so the default is spelled out.
    let rest = std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin"));
    let rest = std::env::split_paths(&rest);
    std::env::join_paths(dirs.into_iter().chain(rest)).ok()
}
