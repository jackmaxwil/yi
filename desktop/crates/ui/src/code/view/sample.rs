//! Sample sources for the scenes and tests: the record's 23-line file, one snippet per language
//! the brief names, and a generator of life-sized Rust.

/// `src/middleware/rate_limit.rs` as the design record prints it (section 6.0).
pub const RATE_LIMIT: &str = "\
use std::time::{Duration, Instant};

const LIMIT: u32 = 20;
const WINDOW: Duration = Duration::from_secs(60);

/// Twenty uploads a minute for each user.
pub struct Bucket {
    tokens: u32,
    refill_at: Instant,
}

impl Bucket {
    pub fn take(&mut self, now: Instant) -> Result<(), Duration> {
        if now >= self.refill_at {
            self.tokens = LIMIT;
            self.refill_at = now + WINDOW;
        }
        match self.tokens.checked_sub(1) {
            Some(left) => { self.tokens = left; Ok(()) }
            None => Err(self.refill_at - now),
        }
    }
}
";

pub const TYPESCRIPT: &str = "\
import { Router } from \"./router\";

/** Twenty uploads a minute for each user. */
export class Bucket {
  private tokens = 20;
  constructor(private readonly refillAt: number) {}

  take(now: number): boolean {
    const label = `bucket ${this.tokens} at ${now}`;
    if (now >= this.refillAt) this.tokens = 20;
    return this.tokens-- > 0 && label.length > 0;
  }
}
";

pub const PYTHON: &str = "\
from dataclasses import dataclass

LIMIT = 20

@dataclass
class Bucket:
    \"\"\"Twenty uploads a minute
    for each user.\"\"\"
    tokens: int = LIMIT

    def take(self, now: float) -> bool:
        # One token for each upload.
        self.tokens -= 1
        return self.tokens >= 0
";

pub const TOML: &str = "\
[package]
name = \"atlas-api\"
version = \"0.4.2\"
edition = \"2024\"

[dependencies]
tokio = { version = \"1\", features = [\"rt-multi-thread\"] }
# Rate limits live in the middleware.
tower = \"0.5\"
";

pub const SHELL: &str = "\
#!/bin/sh
set -eu
# Twenty uploads, then one more.
for i in $(seq 1 21); do
  curl -sf -F \"file=@sample.png\" \"$HOST/upload\" || echo \"limited at $i\"
done
";

pub const JSON: &str = "\
{
  \"name\": \"atlas-api\",
  \"limit\": 20,
  \"window\": 60.0,
  \"routes\": [\"/upload\", \"/health\"],
  \"strict\": true,
  \"owner\": null
}
";

/// Rust source of at least `lines` lines and `bytes` bytes. Every few blocks a block comment
/// or a raw string runs over many lines of text that reads as code, to catch a lexer cut mid-way.
pub fn rust(lines: usize, bytes: usize) -> String {
    let mut out = String::with_capacity(bytes.saturating_add(4096));
    out.push_str("use std::collections::HashMap;\nuse std::time::{Duration, Instant};\n\n");
    let (mut n, mut count) = (0u32, 3usize);
    while count < lines || out.len() < bytes {
        let block = block(n);
        count = count.saturating_add(block.matches('\n').count());
        out.push_str(&block);
        n = n.saturating_add(1);
    }
    out
}

fn block(n: u32) -> String {
    match n % 7 {
        3 => format!(
            "/* Block {n}: a note that runs over several lines.\n   let fake = \"not a string\";\n   \
             fn not_code() {{ return 1; }}\n   // still the same note\n   const ALSO_FAKE: u32 = {n};\n*/\n\n"
        ),
        5 => format!(
            "const TEMPLATE_{n}: &str = r#\"\n    fn inside_a_string() -> u32 {{\n        let x = \"quoted\"; \
             // not a comment\n        {n}\n    }}\n\"#;\n\n"
        ),
        _ => format!(
            "/// Takes one token from bucket {n}, or says how long to wait.\n\
             pub fn take_{n}(buckets: &mut HashMap<u32, Bucket>, now: Instant) -> Result<(), Duration> {{\n    \
             let bucket = buckets.entry({n}).or_insert_with(|| Bucket::new(now));\n    \
             if now >= bucket.refill_at {{\n        bucket.tokens = LIMIT + {n};\n        \
             bucket.refill_at = now + WINDOW;\n    }}\n    \
             match bucket.tokens.checked_sub(1) {{\n        \
             Some(left) => {{ bucket.tokens = left; Ok(()) }}\n        \
             None => Err(bucket.refill_at - now), // \"wait\" for the refill\n    }}\n}}\n\n"
        ),
    }
}
