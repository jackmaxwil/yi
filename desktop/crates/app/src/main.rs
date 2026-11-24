//! Yi Desktop.
#![forbid(unsafe_code)]
#![deny(clippy::string_slice)]

fn main() {
    yi_desktop::start::on_panic(yi_desktop::start::app_log());
    // Invariant: only this binary may say the run is the native app; a test never does.
    yi_desktop::root::run(!rune::is_headless());
}
