//! The built-in `openai-codex` profile yields to a user profile file at the same id. This binary
//! has one test: the file lives under HOME, which the codex login tests read in their own process.

use serde_json::{Value, json};
use yi_oauth::registry::{self, Kind as ProfileKind, Login};

#[allow(dead_code)]
#[path = "common/mod.rs"]
mod common;
use common::Res;

#[test]
fn the_codex_login_is_built_in_and_a_user_profile_file_wins() -> Res {
    let home = std::env::temp_dir().join(format!("yi-codex-profile-{}", std::process::id()));
    std::fs::create_dir_all(&home)?;
    // SAFETY: this binary has one test, so no other thread reads the environment.
    unsafe { std::env::set_var("HOME", &home) };
    let ProfileKind::OauthCode(built_in) = registry::lookup("openai-codex")?.ok_or("built in")?
    else {
        return Err("openai-codex is not an OAuth code profile".into());
    };
    assert_eq!(built_in.client_id, "app_EMoamEEZ73f0CkXaXp7hrann");
    let Login::Code(login) = &built_in.login else {
        return Err("code login".into());
    };
    assert_eq!(login.callback_port, 1455);
    assert!(!login.port_fallback, "the registered redirect is fixed");
    assert!(registry::ids().iter().any(|id| id == "openai-codex"));

    let path = registry::profile_path("openai-codex");
    std::fs::create_dir_all(path.parent().ok_or("profile dir")?)?;
    let custom: Value = json!({
        "kind": "oauth-code",
        "client_id": "mine",
        "authorize": "https://example.invalid/a",
        "token": "https://example.invalid/t",
        "callback_port": 4242
    });
    std::fs::write(&path, custom.to_string())?;
    let looked_up = registry::lookup("openai-codex");
    std::fs::remove_file(&path)?;
    let ProfileKind::OauthCode(user) = looked_up?.ok_or("profile")? else {
        return Err("openai-codex is not an OAuth code profile".into());
    };
    assert_eq!(user.client_id, "mine");
    Ok(())
}
