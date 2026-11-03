fn main() {
    unsafe {
        let _ = std::str::from_utf8_unchecked(b"fixture");
    }
}
