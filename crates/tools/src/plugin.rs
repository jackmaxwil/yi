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
/// record its skills' `sha256sum` as tests/fixtures/ripwire/skills-<version>.sha256.
pub const PLUGINS: [Plugin; 2] = [
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
];

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
            Shape::Program => Some(self.dir(home).join(self.name)),
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
            Shape::Program => crate::syntax::on_path(self.name),
            Shape::Tree => None,
        })
    }
}

pub fn home() -> Option<PathBuf> {
    std::env::var_os("HOME").map(PathBuf::from)
}

/// PATH with every installed program plugin's directory first, so a spawn and the model's bash
/// both find it by name; None when none is installed, which leaves a child's PATH untouched.
pub fn search_path(home: &Path) -> Option<OsString> {
    let dirs: Vec<PathBuf> = PLUGINS
        .iter()
        .filter(|plugin| plugin.shape == Shape::Program && plugin.installed(home).is_some())
        .map(|plugin| plugin.dir(home))
        .collect();
    if dirs.is_empty() {
        return None;
    }
    // Invariant: a child with no PATH searched execvp's default, /usr/bin:/bin; a PATH of the
    // plugin dirs alone would lose both, so the default is spelled out.
    let rest = std::env::var_os("PATH").unwrap_or_else(|| OsString::from("/usr/bin:/bin"));
    let rest = std::env::split_paths(&rest);
    std::env::join_paths(dirs.into_iter().chain(rest)).ok()
}
