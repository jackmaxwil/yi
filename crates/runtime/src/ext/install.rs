use std::path::PathBuf;

use yi_permission::PermissionMode;

use super::grid::Grid;
use super::orchestrate::Orchestrate;
use super::pack::{Pack, PackExtension};
use super::project::{ProjectResources, TrustGate, git_root};
use super::telemetry::RouteTelemetry;
use super::{Host, Rank, Slot, Trust};

const HAR_CORE: &str = include_str!("../prompts/har-core.md");

pub struct ExtOptions {
    pub cwd: PathBuf,
    pub home: PathBuf,
    pub mode: PermissionMode,
    pub user_system: String,
    pub schema_instruction: Option<String>,
    pub context_window: u64,
}

fn rust_pack() -> Pack {
    Pack {
        name: "lang-rust".to_owned(),
        fragment: HAR_CORE.to_owned(),
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
    host.register(Box::new(
        ProjectResources::new(cwd.clone(), home.clone()).with_context_window(context_window),
    ));
    host.register(Box::new(PackExtension::new(rust_pack(), cwd.clone())));
    for pack in user_packs(&cwd, &home) {
        host.register(Box::new(PackExtension::new(pack, cwd.clone())));
    }
    host.register(Box::new(Orchestrate::default()));
    host.register(Box::new(Grid::new(cwd, home)));
    host.register(Box::new(RouteTelemetry::new()));
    host
}

fn user_packs(cwd: &std::path::Path, home: &std::path::Path) -> Vec<Pack> {
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
