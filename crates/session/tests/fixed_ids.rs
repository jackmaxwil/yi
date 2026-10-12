use std::error::Error;

use yi_session::{CreateOptions, IdGenerator, JsonlRepo, SessionRepo, lock_session};

#[path = "../../types/tests/support/scratch.rs"]
mod scratch;
use scratch::Scratch;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn yi_fixed_ids_makes_the_mint_path_deterministic() -> TestResult {
    unsafe { std::env::set_var("YI_FIXED_IDS", "1") };
    let root = Scratch::new("yi-session-fixed-ids")?;
    let mut repo = JsonlRepo::new(root.to_path_buf(), "/tmp");
    let mut direct = IdGenerator::new();
    let repo_fixed = lock_session(&repo.create(CreateOptions::default())?)
        .metadata()
        .id
        .clone();
    let direct_fixed = direct.next_id();
    unsafe { std::env::remove_var("YI_FIXED_IDS") };

    let fresh_root = Scratch::new("yi-session-fixed-ids")?;
    let mut fresh = JsonlRepo::new(fresh_root.to_path_buf(), "/tmp");
    let mut fresh_direct = IdGenerator::new();
    let repo_random = lock_session(&fresh.create(CreateOptions::default())?)
        .metadata()
        .id
        .clone();
    let direct_random = fresh_direct.next_id();

    assert!(
        repo_fixed.starts_with("01a00000-0000-7000-8000-"),
        "{repo_fixed}"
    );
    assert!(
        direct_fixed.starts_with("01a00000-0000-7000-8000-"),
        "{direct_fixed}"
    );
    assert!(!repo_random.starts_with("01a00000-"), "{repo_random}");
    assert!(!direct_random.starts_with("01a00000-"), "{direct_random}");
    Ok(())
}
