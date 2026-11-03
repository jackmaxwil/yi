#[cfg(test)]
mod tests {
    #[test]
    fn temporary_fixture() {
        let _ = std::env::temp_dir();
    }
}

fn main() {
    std::process::Command::new("printf");
}
