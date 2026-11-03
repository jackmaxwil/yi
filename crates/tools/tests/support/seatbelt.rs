#![cfg(target_os = "macos")]

use std::error::Error;

use yi_tools::Sandbox;

/// Seatbelt ships with every supported macOS, so a missing one fails the suite instead of passing it.
pub fn seatbelt() -> Result<(), Box<dyn Error>> {
    match Sandbox::available() {
        true => Ok(()),
        false => Err("Seatbelt is missing on macOS: no command can be contained".into()),
    }
}
