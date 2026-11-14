use std::time::Duration;

fn greeting() -> &'static str {
    "hello from the first build"
}

fn tick() {
    println!("{}", greeting());
}

fn main() {
    subsecond::register_handler(std::sync::Arc::new(|| println!("patched")));
    loop {
        subsecond::call(tick);
        std::thread::sleep(Duration::from_secs(1));
    }
}
