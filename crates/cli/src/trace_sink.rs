//! `YI_TRACE=<dir>`: this process appends its Chrome trace events to `<dir>/<pid>.jsonl`, so
//! every yi process of one run lands in one directory on one timeline.

#[expect(
    clippy::disallowed_methods,
    reason = "the wall anchor is the job: it aligns the processes of one run"
)]
pub(crate) fn install() {
    let Some(dir) = std::env::var_os("YI_TRACE").filter(|dir| !dir.is_empty()) else {
        return;
    };
    let dir = std::path::PathBuf::from(dir);
    let pid = std::process::id();
    let opened = std::fs::create_dir_all(&dir).and_then(|()| {
        std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(dir.join(format!("{pid}.jsonl")))
    });
    let Ok(file) = opened else { return };
    let epoch_us = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX))
        .unwrap_or(0);
    yi_types::trace::install(yi_runtime::trace::file_sink(file), pid, epoch_us);
}
