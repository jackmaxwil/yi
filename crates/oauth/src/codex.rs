use crate::registry::{CodeLogin, Kind, Login, OauthCode};

pub const CODEX_ID: &str = "openai-codex";

/// Incident: the Codex CLI's public client registers exactly `localhost:1455/auth/callback`
/// as its redirect, so the port cannot fall back to a free one.
pub fn profile() -> Kind {
    let pair = |key: &str, value: &str| (key.to_owned(), value.to_owned());
    Kind::OauthCode(Box::new(OauthCode {
        id: CODEX_ID.to_owned(),
        client_id: "app_EMoamEEZ73f0CkXaXp7hrann".to_owned(),
        client_secret: None,
        token: "https://auth.openai.com/oauth/token".to_owned(),
        scopes: "openid profile email offline_access".to_owned(),
        json_token: false,
        stream_headers: Vec::new(),
        refresh_headers: Vec::new(),
        api_base: None,
        login: Login::Code(CodeLogin {
            authorize: "https://auth.openai.com/oauth/authorize".to_owned(),
            callback_port: 1455,
            callback_path: "/auth/callback".to_owned(),
            redirect_host: "localhost".to_owned(),
            port_fallback: false,
            extra_authorize: vec![
                pair("id_token_add_organizations", "true"),
                pair("codex_cli_simplified_flow", "true"),
                pair("originator", "codex_cli_rs"),
            ],
        }),
    }))
}
