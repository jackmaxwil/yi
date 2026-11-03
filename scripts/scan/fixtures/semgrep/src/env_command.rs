use std::process::Command;

fn main() {
    Command::new(std::env::var("PROGRAM").unwrap());
}
