//! A registered scene drawn in process with no GPU, as the window shows it.

use std::error::Error;

use rune::{ElementInfo, Env, Runtime};
use yd_ui::pane::show_scene;
use yd_ui::win::Win;
use yi_desktop::{root, scenes};

/// The window's size in a headless snap, in points.
pub const WINDOW: (f32, f32) = (1200.0, 750.0);

/// The scene `name` after one frame, as a runtime a test can keep driving.
pub fn runtime(name: &str, size: (f32, f32)) -> Result<Runtime<Win>, Box<dyn Error>> {
    let mut win = root::init();
    let index = scenes::all()
        .iter()
        .position(|scene| scene.name == name)
        .ok_or_else(|| format!("no scene {name:?}"))?;
    show_scene(&mut win, index);
    let mut runtime = Runtime::new(win, root::view, size, Env::new(true, 1.0));
    runtime.frame(1.0 / 60.0);
    Ok(runtime)
}

pub fn elements(name: &str) -> Result<Vec<ElementInfo>, Box<dyn Error>> {
    Ok(runtime(name, WINDOW)?.elements())
}
