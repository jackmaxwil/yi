pub(crate) const HALF_MINUTE: std::time::Duration = std::time::Duration::from_secs(30);

#[expect(
    clippy::disallowed_methods,
    reason = "deciding whether the streaming credential has expired is this fn's job"
)]
pub(crate) fn auth_now() -> std::time::SystemTime {
    std::time::SystemTime::now()
}
