use std::io::Write;
use std::sync::Mutex;

struct FileSink(Mutex<std::fs::File>);

impl yi_types::trace::TraceSink for FileSink {
    fn write(&self, event: &str) {
        if let Ok(mut file) = self.0.lock() {
            let _ = file.write_all(format!("{event}\n").as_bytes());
        }
    }
}

pub fn file_sink(file: std::fs::File) -> Box<dyn yi_types::trace::TraceSink> {
    Box::new(FileSink(Mutex::new(file)))
}
