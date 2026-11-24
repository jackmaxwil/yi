//! The window at the narrowest a person keeps it: nothing in a pane's bar lies over anything else.

use super::*;

/// The dogfood window's width, and the narrowest a window may be.
const NARROW: [f32; 2] = [760.0, 720.0];

fn apart(a: [f32; 4], b: [f32; 4]) -> bool {
    a[0] + a[2] <= b[0] + 0.5
        || b[0] + b[2] <= a[0] + 0.5
        || a[1] + a[3] <= b[1] + 0.5
        || b[1] + b[3] <= a[1] + 0.5
}

/// Dies with "Remote desktop" under its split icon and the agent tab's cross on its own icon.
#[test]
fn at_760_points_no_tab_title_or_icon_lies_over_another() -> Check {
    for width in NARROW {
        narrow_bars_keep_apart(width)?;
    }
    Ok(())
}

fn narrow_bars_keep_apart(width: f32) -> Check {
    let mut runtime = unframed("shell/mockup")?;
    runtime.set_viewport((width, 750.0), 1.0);
    rest(&mut runtime);
    let shown = runtime.elements();
    let bars: Vec<&str> = keys(&shown)
        .into_iter()
        .filter(|key| key.ends_with(".pane-tab-bar"))
        .collect();
    assert!(bars.len() >= 7, "a bar a pane: {bars:?}");
    for bar in bars {
        let inside = shown.iter().filter(|e| {
            let drawn = e.visible[2] > 0.5 && e.visible[3] > 0.5;
            !e.exiting && drawn && e.path.contains(bar) && (e.text.is_some() || e.role == "icon")
        });
        let parts: Vec<(String, [f32; 4])> = inside
            .map(|e| (e.text.clone().unwrap_or_else(|| e.path.clone()), e.visible))
            .collect();
        for (at, (one, a)) in parts.iter().enumerate() {
            for (other, b) in parts.iter().skip(at + 1) {
                assert!(
                    apart(*a, *b),
                    "at {width}, in {bar}: {one:?} {a:?} lies over {other:?} {b:?}"
                );
            }
        }
    }
    Ok(())
}
