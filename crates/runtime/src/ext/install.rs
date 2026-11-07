use std::path::{Path, PathBuf};

use yi_permission::PermissionMode;

use super::orchestrate::Orchestrate;
use super::pack::{Pack, PackExtension};
use super::plugins::Plugins;
use super::project::{ProjectResources, TrustGate, git_root};
use super::telemetry::RouteTelemetry;
use super::{Host, Rank, Slot, Trust};

pub(crate) static ORCHESTRATE: std::sync::LazyLock<String> = crate::packed::packed!("orchestrate");
pub(crate) static HAR_CORE: std::sync::LazyLock<String> = crate::packed::packed!("har-core");

pub struct ExtOptions {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub mode: PermissionMode,
    pub user_system: String,
    pub schema_instruction: Option<String>,
    pub context_window: u64,
    pub global_skills: Vec<String>,
    pub pointer: Option<String>,
    /// Invariant: false where the depth cap already refuses this session's spawns, so the
    /// protocol for delegating is never sent to a session that cannot act on it.
    pub can_delegate: bool,
}

fn rust_pack() -> Pack {
    Pack {
        name: "lang-rust".to_owned(),
        fragment: HAR_CORE.as_str().to_owned(),
        project_markers: vec!["Cargo.toml".to_owned()],
        write_extensions: vec!["rs".to_owned()],
    }
}

pub fn install(options: ExtOptions) -> Host {
    let ExtOptions {
        cwd,
        home,
        mode,
        user_system,
        schema_instruction,
        context_window,
        global_skills,
        pointer,
        can_delegate,
    } = options;
    let mut host = Host::new(cwd.clone());
    host.attach(
        Slot::new(Rank::Identity, "identity"),
        crate::identity_fragment().to_owned(),
    );
    host.attach(
        Slot::new(Rank::Doctrine, "doctrine"),
        crate::doctrine_fragment().to_owned(),
    );
    if can_delegate {
        host.attach(
            Slot::new(Rank::Doctrine, "delegation"),
            crate::delegation_fragment().to_owned(),
        );
    }
    host.attach(
        Slot::new(Rank::Mode, "permission"),
        yi_permission::mode_fragment(mode).to_owned(),
    );
    if !user_system.trim().is_empty() {
        host.attach(Slot::new(Rank::User, "system"), user_system);
    }
    if let Some(schema) = schema_instruction {
        host.attach(Slot::new(Rank::Schema, "schema"), schema);
    }
    let resources = ProjectResources::new(cwd.clone(), home.clone())
        .with_context_window(context_window)
        .with_global_skills(global_skills);
    host.register(Box::new(match pointer {
        Some(pointer) => resources.with_pointer(pointer),
        None => resources,
    }));
    register_packs(&mut host, &cwd, &home);
    if can_delegate {
        host.register(Box::new(Orchestrate::new(ORCHESTRATE.as_str())));
    }
    host.register(Box::new(Plugins { home }));
    host.register(Box::new(RouteTelemetry::new()));
    host
}

pub fn narrow(cwd: &Path, home: &Path, identity: &str, pointer: Option<String>) -> Host {
    let mut host = Host::new(cwd.to_path_buf());
    host.attach(Slot::new(Rank::Identity, "identity"), identity.to_owned());
    let resources = ProjectResources::new(cwd.to_path_buf(), home.to_path_buf());
    host.register(Box::new(match pointer {
        Some(pointer) => resources.with_pointer(pointer),
        None => resources,
    }));
    register_packs(&mut host, cwd, home);
    host
}

fn register_packs(host: &mut Host, cwd: &Path, home: &Path) {
    host.register(Box::new(PackExtension::new(rust_pack(), cwd.to_path_buf())));
    for pack in user_packs(cwd, home) {
        host.register(Box::new(PackExtension::new(pack, cwd.to_path_buf())));
    }
}

fn user_packs(cwd: &Path, home: &Path) -> Vec<Pack> {
    let mut packs = Pack::load_dir(&home.join(".yi/extensions"));
    let root = git_root(cwd).unwrap_or_else(|| cwd.to_path_buf());
    let gate = TrustGate::new(home);
    for (name, body) in super::project::pack_files(cwd) {
        if gate.trust_of(&root, &format!("extensions/{name}"), &body) != Trust::Granted {
            continue;
        }
        if let Some(pack) = Pack::load(&cwd.join(".yi/extensions").join(&name)) {
            packs.push(pack);
        }
    }
    packs
}
