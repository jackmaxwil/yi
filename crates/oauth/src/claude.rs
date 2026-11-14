use crate::registry::{Kind, Login, OauthCode, PasteLogin};

pub const CLAUDE_ID: &str = "claude";
pub const ANTHROPIC_ID: &str = "anthropic";
pub const BETA_HEADER: (&str, &str) = ("anthropic-beta", "oauth-2025-04-20");
pub const IDENTITY_LINE: &str = "You are Claude Code, Anthropic's official CLI for Claude.";

/// Incident: the Claude subscription's public client registers only the console's own
/// code page as its redirect, so the user pastes `code#state` back instead of a loopback hop.
pub fn profile() -> Kind {
    Kind::OauthCode(Box::new(OauthCode {
        id: ANTHROPIC_ID.to_owned(),
        client_id: "9d1c250a-e61b-44d9-88ed-5944d1962f5e".to_owned(),
        client_secret: None,
        token: "https://console.anthropic.com/v1/oauth/token".to_owned(),
        scopes: "org:create_api_key user:profile user:inference".to_owned(),
        json_token: true,
        stream_headers: vec![(BETA_HEADER.0.to_owned(), BETA_HEADER.1.to_owned())],
        refresh_headers: Vec::new(),
        api_base: None,
        login: Login::Paste(PasteLogin {
            authorize: "https://claude.ai/oauth/authorize".to_owned(),
            redirect_uri: "https://console.anthropic.com/oauth/code/callback".to_owned(),
        }),
    }))
}
