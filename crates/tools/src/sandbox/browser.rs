use std::path::Path;

use super::{SEATBELT, Sandbox, temp_roots};

/// What Chrome's own processes need past the base policy, each found by the failure without it:
/// `bootstrap_check_in … Permission denied (1100)` from its Mach rendezvous.
const BROWSER_POLICY: &str = "; Chrome's browser and child processes\n\
    (allow mach-lookup)\n\
    (allow mach-register (global-name-prefix \"org.chromium.\"))\n\
    (allow ipc-posix-shm*)\n\
    (allow sysctl-read)\n";

/// The internet by address, and of unix sockets only the resolver's: an ssh agent's or a Docker
/// daemon's would be a way off the box. With no resolver every name fails `ERR_NAME_NOT_RESOLVED`.
const WEB_POLICY: &str = "; the internet\n(allow network-outbound (remote ip \"*:*\"))\n\
    (allow network-outbound (literal \"/private/var/run/mDNSResponder\"))\n";

impl Sandbox {
    /// Chrome's box: it reads what a contained command reads and writes only temp, and no tree.
    pub fn for_browser(home: &Path) -> Self {
        Self {
            writable: temp_roots(),
            deny_read: yi_permission::credential_stores(home),
            deny_write: Vec::new(),
            sealed: Vec::new(),
            host_owned: vec![home.join(".yi/sessions")],
            spared: Vec::new(),
        }
    }

    /// Chrome in this box, its own sandbox off since it cannot nest: the GPU process dies, and with
    /// no GPU the renderer never answers. `env -i`, so it inherits none of the daemon's variables.
    pub fn wrap_browser(&self, web: bool, home: &Path, chrome: &str) -> (String, Vec<String>) {
        let web = if web { WEB_POLICY } else { "" };
        let mut wrapped = self.bound_policy(None, true, &format!("{BROWSER_POLICY}{web}"));
        wrapped.extend(
            [
                "/usr/bin/env",
                "-i",
                &format!("HOME={}", home.display()),
                chrome,
                "--no-sandbox",
            ]
            .map(str::to_owned),
        );
        (SEATBELT.to_owned(), wrapped)
    }
}
