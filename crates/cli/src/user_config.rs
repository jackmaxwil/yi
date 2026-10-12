//! The user's `config.json`: loaded strictly once before dispatch, and read again as each
//! session starts.

use std::sync::Arc;

use yi_types::config::{ConfigMigration, UserConfig};

use crate::{Args, setup, update};

/// Yi has no built-in default model; the user's config carries it, either as
/// `"model"` or as the §5 `"models"` role table.
pub(crate) fn configured_model() -> Option<String> {
    let roles = configured_roles();
    if let Some(primary) = roles.primary {
        return Some(primary);
    }
    config().model.clone()
}

static CONFIG: std::sync::RwLock<Option<Arc<UserConfig>>> = std::sync::RwLock::new(None);
static HOME: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// The one config load, strict, before dispatch — a typo that reads as an unset default
/// is the failure nobody sees. It lives here because no fs or `$HOME` may reach yi-types.
pub(crate) fn load_config() -> Result<(), String> {
    let home = update::absolute_home()?;
    let home = HOME.get_or_init(|| home);
    setup::early();
    yi_runtime::set_catalog_cache_dir(home.join(".yi/catalog"));
    let (config, migrations) = read_config(home)?;
    for migration in migrations {
        eprintln!("warning: {migration}");
    }
    set_config(config)
}

/// A relative home would read `<cwd>/.yi/config.json`, which is §17.1's project
/// layer, not this one; [`load_config`] is the only caller for that reason.
pub(crate) fn read_config(
    home: &std::path::Path,
) -> Result<(UserConfig, Vec<ConfigMigration>), String> {
    let path = home.join(".yi/config.json");
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(Default::default());
        }
        Err(error) => return Err(format!("{}: {error}", path.display())),
    };
    yi_types::config::parse(&raw).map_err(|error| format!("{}: {error}", path.display()))
}

/// Invariant: nothing may read the config before this lands it, or the strict
/// load would be a no-op behind a default somebody else already installed.
pub(crate) fn set_config(loaded: UserConfig) -> Result<(), String> {
    let mut slot = CONFIG
        .write()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    if slot.is_some() {
        return Err("config was already read before it was loaded".to_owned());
    }
    *slot = Some(Arc::new(loaded));
    Ok(())
}

pub(crate) fn config() -> Arc<UserConfig> {
    let read = CONFIG.read().ok().and_then(|slot| slot.clone());
    read.unwrap_or_else(|| {
        let mut slot = CONFIG
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        slot.get_or_insert_with(Arc::default).clone()
    })
}

/// Invariant: a session starts on the file as it is now, since the settings page saves it while
/// the worker that builds the next session runs; a file that no longer loads keeps the last.
pub(crate) fn reload_config() {
    match read_config(home()) {
        Ok((fresh, _)) => {
            *CONFIG
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::new(fresh));
        }
        Err(error) => eprintln!("warning: {error}; this session keeps the config read before"),
    }
}

pub(crate) fn configured_mode() -> yi_runtime::PermissionMode {
    match config()
        .permissions
        .as_ref()
        .and_then(|permissions| permissions.mode)
    {
        Some(yi_types::config::ModeName::Ask) => yi_runtime::PermissionMode::Ask,
        Some(yi_types::config::ModeName::Yolo) => yi_runtime::PermissionMode::Yolo,
        Some(yi_types::config::ModeName::Auto) | None => yi_runtime::PermissionMode::Auto,
    }
}

/// The flags the command line did not pin, read again from the config [`reload_config`] left.
pub(crate) fn refreshed(args: &Args) -> Args {
    let mut args = args.clone();
    if !args.model_pinned {
        args.model = configured_model().unwrap_or_default();
    }
    if !args.thinking_pinned {
        args.thinking = config().thinking;
    }
    if !args.mode_pinned {
        args.mode = configured_mode();
    }
    args
}

/// Invariant: the HOME [`load_config`] checked; `doctor` reports it raw, `plugin` checks it itself.
pub(crate) fn home() -> &'static std::path::Path {
    HOME.get_or_init(|| std::env::var_os("HOME").map(Into::into).unwrap_or_default())
}

pub(crate) fn configured_roles() -> yi_types::config::ModelRoles {
    config().models.clone().unwrap_or_default()
}

pub(crate) fn configured_auto_background() -> Option<std::time::Duration> {
    let millis = config().bash.as_ref()?.auto_background_ms?;
    (millis > 0).then(|| std::time::Duration::from_millis(millis))
}
