#![forbid(unsafe_code)]

fn main() {
    let version = env!("CARGO_PKG_VERSION");
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "version") => println!("yi {version}"),
        _ => println!("yi {version} (phase 0 scaffold; surfaces land in phase 1)"),
    }
}
