use crate::flow::{self, LoginOptions};
use crate::registry;
use crate::store::Store;

fn env_proxy() -> Result<Option<ureq::Proxy>, String> {
    let url = ["HTTPS_PROXY", "HTTP_PROXY"]
        .into_iter()
        .find_map(|name| std::env::var(name).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    let Some(url) = url else {
        return Ok(None);
    };
    if url.starts_with("socks") {
        return Err(format!("SOCKS proxies are not supported: {url}"));
    }
    ureq::Proxy::new(&url)
        .map(Some)
        .map_err(|error| format!("invalid proxy: {error}"))
}

fn usage() -> i32 {
    eprintln!("usage: yi login [--list] <provider> [--no-browser] [--key <secret>]");
    eprintln!("       yi logout [provider]");
    eprintln!(
        "profiles: ~/.yi/oauth/<provider>.json (yi ships none; see docs/examples/oauth-profile.json)"
    );
    eprintln!("examples: yi login --list");
    eprintln!("          yi login openai            # paste the key at the prompt");
    eprintln!(
        "          yi login openai < ~/.key   # or pipe it: a --key value shows in ps and shell history"
    );
    eprintln!("          yi login <provider-with-a-profile> --no-browser");
    2
}

fn print_list() -> i32 {
    for id in registry::ids() {
        println!("{id}");
    }
    0
}

fn login(args: &[String]) -> i32 {
    let mut no_browser = false;
    let mut list = false;
    let mut provider = None;
    let mut key = None;
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--list" => list = true,
            "--no-browser" => no_browser = true,
            "--help" | "-h" => return usage(),
            "--key" => {
                let Some(value) = rest.next() else {
                    eprintln!("error: --key needs a value");
                    return usage();
                };
                key = Some(value.clone());
            }
            other if other.starts_with('-') => {
                if let Some(value) = other.strip_prefix("--key=") {
                    key = Some(value.to_owned());
                } else {
                    eprintln!("error: unknown flag {other}");
                    return 2;
                }
            }
            other => provider = Some(other.to_owned()),
        }
    }
    if list {
        return print_list();
    }
    let Some(provider) = provider else {
        return usage();
    };
    let kind = match registry::lookup(&provider) {
        Ok(Some(kind)) => kind,
        Ok(None) => {
            eprintln!(
                "error: no login profile for {provider}. Write one at {} (see docs/examples/oauth-profile.json), or try --list.",
                registry::profile_path(&provider).display()
            );
            return 2;
        }
        Err(broken) => {
            eprintln!("error: {broken}");
            return 2;
        }
    };
    let proxy = match env_proxy() {
        Ok(proxy) => proxy,
        Err(error) => {
            eprintln!("error: {error}");
            return 2;
        }
    };
    if key.is_some() && !matches!(kind, registry::Kind::ApiKey) {
        eprintln!("error: --key is only for pasted API keys (openai, openrouter, google)");
        return 2;
    }
    let result = match kind {
        registry::Kind::OauthCode(spec) => {
            eprintln!(
                "warning: {provider} logs in with a profile you supplied ({}). Yi ships no client id or wire identity: whatever that file carries is sent as-is, and a provider's terms may forbid it. API keys remain the supported path.",
                registry::profile_path(&provider).display()
            );
            flow::login_oauth(&spec, &LoginOptions { no_browser }, proxy.as_ref())
        }
        registry::Kind::ApiKey => {
            let pasted = if let Some(key) = key {
                key
            } else {
                eprint!("API key for {provider}: ");
                let mut key = String::new();
                if std::io::stdin().read_line(&mut key).is_err() {
                    eprintln!("error: failed to read key");
                    return 2;
                }
                key
            };
            flow::login_api_key(pasted)
        }
    };
    match result.and_then(|credential| flow::save(&provider, &credential)) {
        Ok(()) => {
            println!("logged in as {provider}");
            0
        }
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

fn logout(args: &[String]) -> i32 {
    if args.is_empty() {
        // The token directory, not the profile registry: a provider whose profile was
        // deleted still has its token removed.
        let mut failed = false;
        for id in Store::user().list() {
            if let Err(error) = flow::logout(&id) {
                eprintln!("error: {error}");
                failed = true;
            }
        }
        return i32::from(failed);
    }
    if args.len() != 1 {
        return usage();
    }
    match flow::logout(&args[0]) {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("error: {error}");
            1
        }
    }
}

pub fn run(verb: &str, args: &[String]) -> i32 {
    match verb {
        "login" => login(args),
        "logout" => logout(args),
        _ => usage(),
    }
}
