//! A `local://` address names a path with a space as `%20`, the spelling a model copies from a
//! listing, and prints back the bytes it was given.
use std::error::Error;

use yi_types::url::Url;

type TestResult = Result<(), Box<dyn Error>>;

#[test]
fn a_local_address_carries_a_space_as_percent_twenty() -> TestResult {
    let text = "local://dir%20x/f.txt";
    let url: Url = text.parse()?;
    assert_eq!(
        (url.path(), url.to_string().as_str()),
        ("dir x/f.txt", text)
    );
    let raw: Result<Url, _> = "local://dir x/f.txt".parse();
    assert!(raw.is_err(), "a raw space is still refused: {raw:?}");
    let other: Url = "plan://p%20q".parse()?;
    assert_eq!(other.path(), "p%20q", "only local:// decodes");
    Ok(())
}
