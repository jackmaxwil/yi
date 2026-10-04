use std::error::Error;
use yi_types::image::{ImageDefect, image_defect};

// Pillow 12.3 encodes; the cap image is the 1x1 PNG carrying a 7,499,913-byte tEXt chunk.
const PNG: &str =
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGM4+3g/AATwAnDE8Xs+AAAAAElFTkSuQmCC";
const JPEG: &str = "/9j/4AAQSkZJRgABAQAAAQABAAD/2wBDAAgGBgcGBQgHBwcJCQgKDBQNDAsLDBkSEw8UHRofHh0aHBwgJC4nICIsIxwcKDcpLDAxNDQ0Hyc5PTgyPC4zNDL/2wBDAQkJCQwLDBgNDRgyIRwhMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjIyMjL/wAARCAABAAEDASIAAhEBAxEB/8QAHwAAAQUBAQEBAQEAAAAAAAAAAAECAwQFBgcICQoL/8QAtRAAAgEDAwIEAwUFBAQAAAF9AQIDAAQRBRIhMUEGE1FhByJxFDKBkaEII0KxwRVS0fAkM2JyggkKFhcYGRolJicoKSo0NTY3ODk6Q0RFRkdISUpTVFVWV1hZWmNkZWZnaGlqc3R1dnd4eXqDhIWGh4iJipKTlJWWl5iZmqKjpKWmp6ipqrKztLW2t7i5usLDxMXGx8jJytLT1NXW19jZ2uHi4+Tl5ufo6erx8vP09fb3+Pn6/8QAHwEAAwEBAQEBAQEBAQAAAAAAAAECAwQFBgcICQoL/8QAtREAAgECBAQDBAcFBAQAAQJ3AAECAxEEBSExBhJBUQdhcRMiMoEIFEKRobHBCSMzUvAVYnLRChYkNOEl8RcYGRomJygpKjU2Nzg5OkNERUZHSElKU1RVVldYWVpjZGVmZ2hpanN0dXZ3eHl6goOEhYaHiImKkpOUlZaXmJmaoqOkpaanqKmqsrO0tba3uLm6wsPExcbHyMnK0tPU1dbX2Nna4uPk5ebn6Onq8vP09fb3+Pn6/9oADAMBAAIRAxEAPwD1iiiiuMwP/9k=";
const GIF: &str = "R0lGODdhAQABAIEAAMgeHgAAAAAAAAAAACwAAAAAAQABAAAIBAABBAQAOw==";
const WEBP: &str =
    "UklGRjoAAABXRUJQVlA4IC4AAACwAQCdASoBAAEAAUAmJaACdLoABDAAAP7x3I/4DdfFtMv/vYL/3YL/3YL/WwAA";
const BMP: &str =
    "Qk06AAAAAAAAADYAAAAoAAAAAQAAAAEAAAABABgAAAAAAAQAAADEDgAAxA4AAAAAAAAAAAAAv+PNAA==";
const PNG_8000_WIDE: &str = "iVBORw0KGgoAAAANSUhEUgAAH0AAAAABAQAAAAB7GKW3AAAAE0lEQVR4nGP8zzAKRsEoYBjmAADsFwEBuSBOMwAAAABJRU5ErkJggg==";
const PNG_8001_WIDE: &str = "iVBORw0KGgoAAAANSUhEUgAAH0EAAAABAQAAAACU2s6JAAAAFElEQVR4nGP8zzAKRsEoYBjmoBEA7ZkBghYDe9wAAAAASUVORK5CYII=";
const PNG_8001_TALL: &str = "iVBORw0KGgoAAAANSUhEUgAAAAEAAB9BAQAAAACpoB9vAAAAXklEQVR4nO3QAQ0AAAzDoGbKL/1GCA7oFgYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgwIABAwYMGDBgYD1R0T8BEkoctQAAAABJRU5ErkJggg==";
const GIF_8001_WIDE: &str = "R0lGODdhQR8BAIAAAAAAAAAAACwAAAAAQR8BAAAIkAABCBxIsKDBgwgTKlzIsKHDhxAjSpxIsaLFixgzatzIsaPHjyBDihxJsqTJkyhTqlzJsqXLlzBjypxJs6bNmzhz6tzJs6fPn0CDCh1KtKjRo0iTKl3KtKnTp1CjSp1KtarVq1izat3KtavXr2DDih1LtqzZs2jTql3Ltq3bt3Djyp1Lt67du3jz6t3Lt6/fgAA7";

fn png_with_text(head: &str, groups: usize, tail: &str) -> String {
    format!("{head}{}{tail}", "QUFB".repeat(groups))
}

/// Incident: a provider refuses these on every later request once they are in history (#860);
/// each limit is taken at its value and one step past it.
#[test]
fn an_image_is_refused_for_what_the_provider_refuses() -> Result<(), Box<dyn Error>> {
    let at_cap = png_with_text(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAHJwjXRFWHRrZXkA",
        2_499_971,
        "YaWqugAAAAxJREFUeJxjOPt4PwAE8AJwxPF7PgAAAABJRU5ErkJggg==",
    );
    let past_cap = png_with_text(
        "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAHJwkHRFWHRrZXkA",
        2_499_972,
        "XD1JBAAAAAxJREFUeJxjOPt4PwAE8AJwxPF7PgAAAABJRU5ErkJggg==",
    );
    assert_eq!((at_cap.len(), past_cap.len()), (10_000_000, 10_000_004));
    let cases = [
        ("image/png", PNG, None),
        ("image/jpeg", JPEG, None),
        ("image/gif", GIF, None),
        ("image/webp", WEBP, None),
        ("image/png", at_cap.as_str(), None),
        (
            "image/png",
            past_cap.as_str(),
            Some(ImageDefect::TooLarge { chars: 10_000_004 }),
        ),
        ("image/png", PNG_8000_WIDE, None),
        (
            "image/png",
            PNG_8001_WIDE,
            Some(ImageDefect::TooWide {
                width: 8001,
                height: 1,
            }),
        ),
        (
            "image/png",
            PNG_8001_TALL,
            Some(ImageDefect::TooWide {
                width: 1,
                height: 8001,
            }),
        ),
        (
            "image/gif",
            GIF_8001_WIDE,
            Some(ImageDefect::TooWide {
                width: 8001,
                height: 1,
            }),
        ),
        ("image/bmp", BMP, Some(ImageDefect::Type)),
        ("image/png", JPEG, Some(ImageDefect::NotItsType)),
        (
            "image/png",
            "iVBORw0KGgoAAAAA",
            Some(ImageDefect::Truncated),
        ),
        (
            "image/png",
            PNG.get(1..).ok_or("png")?,
            Some(ImageDefect::NotBase64),
        ),
    ];
    for (mime_type, data, defect) in cases {
        assert_eq!(
            image_defect(mime_type, data),
            defect,
            "{mime_type} {data:.40}"
        );
    }
    Ok(())
}

/// A real encode cut at any four-character boundary is still strict base64 with a good head,
/// which is what the kernel's check passed; only the end marker shows the cut.
#[test]
fn an_image_cut_short_is_refused() -> Result<(), Box<dyn Error>> {
    for (mime_type, data) in [
        ("image/png", PNG),
        ("image/jpeg", JPEG),
        ("image/gif", GIF),
        ("image/webp", WEBP),
    ] {
        for cut in (16..data.len()).step_by(4) {
            let head = data.get(..cut).ok_or("cut")?;
            assert_eq!(
                image_defect(mime_type, head),
                Some(ImageDefect::Truncated),
                "{mime_type} at {cut}"
            );
        }
    }
    Ok(())
}
