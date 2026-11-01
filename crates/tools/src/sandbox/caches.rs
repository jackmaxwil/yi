//! The package stores a contained install may write.

use std::path::{Path, PathBuf};

use super::Sandbox;

impl Sandbox {
    /// The package stores a contained install writes, where `CARGO_HOME` and `UV_CACHE_DIR` put
    /// them.
    #[must_use]
    pub fn with_caches(self, home: &Path) -> Self {
        let from_env = |key: &str| {
            let dir = std::env::var_os(key).filter(|dir| !dir.is_empty());
            dir.map(PathBuf::from)
        };
        self.with_caches_at(home, from_env("CARGO_HOME"), from_env("UV_CACHE_DIR"))
    }

    /// Never `.cargo/bin` or a config, which the host runs and reads. Invariant: a store that is
    /// `home`, holds it or holds a credential store stays read-only: `CARGO_HOME=$HOME` widens nothing.
    #[must_use]
    pub fn with_caches_at(
        mut self,
        home: &Path,
        cargo: Option<PathBuf>,
        uv: Option<PathBuf>,
    ) -> Self {
        let cargo = cargo.unwrap_or_else(|| home.join(".cargo"));
        let uv = uv.unwrap_or_else(|| home.join(".cache/uv"));
        let stores = yi_permission::credential_stores(home);
        let safe = |root: &PathBuf| {
            !home.starts_with(root)
                && !(stores.iter()).any(|store| store.starts_with(root) || root.starts_with(store))
        };
        let cargo_leaves = [
            "registry",
            "git",
            ".package-cache",
            ".package-cache-mutate",
            ".global-cache",
        ];
        let home_leaves = [
            ".npm",
            "Library/pnpm",
            "Library/Caches/pnpm",
            "Library/Caches/pip",
            ".bun/install/cache",
        ];
        let cargo_caches = (cargo_leaves.iter())
            .filter(|_| !home.starts_with(&cargo))
            .map(|leaf| cargo.join(leaf));
        let caches = (home_leaves.iter()).map(|leaf| home.join(leaf)).chain([uv]);
        let writable: Vec<PathBuf> = cargo_caches.chain(caches).filter(safe).collect();
        self.writable.extend(writable);
        self
    }
}
