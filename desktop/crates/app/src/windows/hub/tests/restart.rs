//! What a restart opens again: the windows, the thread, the sidebar and each checkout.

use std::error::Error;
use std::fs;
use std::path::PathBuf;

use rune::Tx;
use yd_core::pane::PaneKind;
use yd_core::wire::replay::play;
use yd_ui::grid_view::view::{Act, act};

use super::super::*;
use super::{LISTED, OUT, OWN, TWO, WEB_PANE, desk, display, hub, leave_web, listed, window};
use crate::windows::hub::scenes::{HOME, docked};

/// A run over `file` whose pane went out, then `close` on its window, and the next run's specs.
fn closed_then_restarted(
    name: &str,
    close: fn(&mut Win, &mut Tx),
) -> Result<usize, Box<dyn Error>> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wn")
        .join(format!("{name}-{}.json", std::process::id()));
    let laptop = [display((0, 0), (2400, 1500), 2.0, true)];
    let launch = || Launch {
        headless: false,
        places: Places::at(&file),
    };
    let (first, _) = hub(|_| {})?;
    let _specs = specs(&first, &laptop, launch());
    leave_web(&first)?;
    let mut main = window(&first, MAIN);
    main.frame(1.0 / 60.0);
    let mut own = window(&first, OWN);
    own.frame(1.0 / 60.0);
    own.update(close);
    own.frame(1.0 / 60.0);
    main.frame(1.0 / 60.0);
    drop((main, own));
    let (next, _) = hub(|_| {})?;
    let specs = specs(&next, &laptop, launch());
    let _ = fs::remove_file(&file);
    Ok(specs.len())
}

#[test]
fn a_pulled_out_pane_closed_by_its_cross_does_not_come_back_after_a_restart()
-> Result<(), Box<dyn Error>> {
    let cross = |win: &mut Win, tx: &mut Tx| act(win, tx, Act::Close(WEB_PANE));
    assert_eq!(closed_then_restarted("cross", cross)?, 1, "the app alone");
    assert_eq!(closed_then_restarted("red", closed)?, 1, "the app alone");
    Ok(())
}

#[test]
fn a_settings_window_open_when_the_app_was_killed_does_not_open_by_itself_next_run()
-> Result<(), Box<dyn Error>> {
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wn")
        .join(format!("apart-{}.json", std::process::id()));
    let laptop = [display((0, 0), (2400, 1500), 2.0, true)];
    let launch = || Launch {
        headless: false,
        places: Places::at(&file),
    };
    let (first, _) = hub(|_| {})?;
    let _specs = specs(&first, &laptop, launch());
    let mut main = window(&first, MAIN);
    main.update(|win, _| {
        apart(win, PaneKind::Settings);
    });
    main.frame(1.0 / 60.0);
    assert_eq!(first.try_borrow()?.model.grid.windows.len(), 2, "it opened");
    drop(main);
    let (next, _) = hub(|_| {})?;
    let specs = specs(&next, &laptop, launch());
    let _ = fs::remove_file(&file);
    assert_eq!(specs.len(), 1, "the app alone");
    let hub = next.try_borrow()?;
    let settings = hub
        .model
        .grid
        .panes
        .iter()
        .find(|p| p.kind == PaneKind::Settings);
    let out = settings.and_then(|pane| hub.model.grid.window_of(pane.id));
    assert_eq!(out, None, "nor is it drawn in the app");
    Ok(())
}

#[test]
fn a_pane_pulled_out_where_the_desk_has_room_opens_beside_the_app_not_over_it()
-> Result<(), Box<dyn Error>> {
    let events = format!(r#"{OUT},"t":0.3}},{{"t":1.0,"type":"mark","name":"end"}}"#);
    let ran = desk("pulled-beside", TWO, docked, &events)?;
    let own = ran.window("window-1")?.frame;
    assert!(
        own[0] >= HOME.x + HOME.w,
        "it covers none of the app: {own:?}"
    );
    assert_eq!(own[1], HOME.y, "level with the app's top");
    Ok(())
}

/// The same list as a client that resumes it hears it: Yi says nothing of the checkout.
fn listed_again(hub: &mut Hub) {
    let lines = LISTED
        .lines()
        .take(29)
        .filter(|line| !line.contains("_yi/workdir"));
    assert!(play(&mut hub.model, &lines.collect::<Vec<_>>().join("\n")).is_ok());
}

#[test]
fn a_restart_shows_the_thread_sidebar_and_checkout_the_app_left_with() -> Result<(), Box<dyn Error>>
{
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wn")
        .join(format!("shown-{}.json", std::process::id()));
    let laptop = [display((0, 0), (2400, 1500), 2.0, true)];
    let launch = || Launch {
        headless: false,
        places: Places::at(&file),
    };
    let (first, _) = hub(listed)?;
    let _specs = specs(&first, &laptop, launch());
    let (picked, worker, lane) = {
        let mut hub = first.try_borrow_mut()?;
        let roots = shown::roots(&hub.model.wire);
        let last = roots.last().ok_or("no thread")?.id.clone();
        let thread = roots
            .into_iter()
            .find(|t| t.workdir.is_some())
            .ok_or("no lane")?;
        let (worker, lane) = (thread.id.clone(), shown::works_in(&hub, thread).to_owned());
        assert_ne!(
            shown::thread(&hub, MAIN).map(|t| &t.id),
            Some(&last),
            "not the first"
        );
        shown::pick(&mut hub, MAIN, last.clone());
        crate::shell::view::change(&mut hub, MAIN, |shell| {
            shell.hidden = true;
            shell.width = Some(260.0);
        });
        (last, worker, lane)
    };
    window(&first, MAIN).frame(1.0 / 60.0);
    let (next, _) = hub(listed_again)?;
    let _specs = specs(&next, &laptop, launch());
    let _ = fs::remove_file(&file);
    let hub = next.try_borrow()?;
    let thread = shown::thread(&hub, MAIN).ok_or("no thread shows")?;
    assert_eq!(thread.id, picked, "the thread it showed");
    let worker = hub.model.wire.threads.get(&worker).ok_or("gone")?;
    assert_eq!(shown::works_in(&hub, worker), lane, "in its checkout");
    assert!(
        crate::shell::view::shell(&hub, MAIN).hidden,
        "the sidebar hidden"
    );
    let width = crate::shell::view::shell(&hub, MAIN).width;
    assert_eq!(width, Some(260.0), "as wide as it was");
    Ok(())
}

/// A layout kept and then renamed comes back, with its number, after the app is killed:
/// the rename moves nothing a pane draws, so it is kept once the grid rests.
#[test]
fn a_layout_kept_and_renamed_comes_back_after_a_restart() -> Result<(), Box<dyn Error>> {
    use yd_core::grid::saved::OwnId;
    let file = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/wn")
        .join(format!("kept-layout-{}.json", std::process::id()));
    let laptop = [display((0, 0), (2400, 1500), 2.0, true)];
    let launch = || Launch {
        headless: false,
        places: Places::at(&file),
    };
    let (first, _) = hub(|_| {})?;
    let _specs = specs(&first, &laptop, launch());
    let mut main = window(&first, MAIN);
    main.update(|win, _| {
        let name = "Draft".to_owned();
        dispatch(win, Msg::Grid(GridMsg::Keep { window: MAIN, name }));
    });
    main.frame(1.0 / 60.0);
    main.update(|win, _| {
        let name = "Reading".to_owned();
        dispatch(
            win,
            Msg::Grid(GridMsg::Rename {
                own: OwnId(6),
                name,
            }),
        );
    });
    for _ in 0..3 {
        main.frame(0.6);
    }
    drop(main);
    let (next, _) = hub(|_| {})?;
    let _specs = specs(&next, &laptop, launch());
    let _ = fs::remove_file(&file);
    let hub = next.try_borrow()?;
    let kept: Vec<(u8, &str)> = hub
        .model
        .grid
        .own
        .iter()
        .map(|o| (o.id.0, o.name.as_str()))
        .collect();
    assert_eq!(kept, [(6, "Reading")]);
    let shown = hub.model.grid.window(MAIN).and_then(|window| window.own);
    assert_eq!(shown, Some(OwnId(6)), "and the window still shows it");
    Ok(())
}
