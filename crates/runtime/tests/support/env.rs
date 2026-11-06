/// Restores an environment variable to its prior value, or to unset, when dropped.
pub struct EnvGuard(&'static str, Option<std::ffi::OsString>);

impl EnvGuard {
    pub fn new(name: &'static str) -> Self {
        Self(name, std::env::var_os(name))
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        // SAFETY: tests use this only in an isolated process or the serial Cargo fallback.
        unsafe {
            match &self.1 {
                Some(value) => std::env::set_var(self.0, value),
                None => std::env::remove_var(self.0),
            }
        }
    }
}
