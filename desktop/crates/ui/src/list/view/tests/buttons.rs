//! The buttons of an opened row: the kit's button, its press-in, its key hint, and which one
//! is the main one and which are quiet.

use rune::{Cx, El};
use yd_core::list::{Detail, ListDesc, Main};
use yd_core::pane::PaneKind;
use yd_core::row::Row;
use yd_core::tokens::Tokens;

use super::{
    Check, DAEMONS, RESTART, SECOND, SERVICES, five, keyed, list, pane, press, says, service_key,
    services, window,
};
use crate::theme::look::colour;
use crate::win::{Hub, Win};

const RESTART_KEY: &str = "daemons-7.detail.b.daemons.restart";
const ATTACH_KEY: &str = "daemons-7.detail.b.context.attach";

fn marked(_: &Row) -> Detail {
    Detail {
        commands: vec![RESTART, SECOND],
        main: Main::Command(SECOND),
        quiet: vec![RESTART],
        ..Detail::default()
    }
}

fn plain(_: &Row) -> Detail {
    Detail {
        commands: vec![RESTART, SECOND],
        main: Main::Neither,
        ..Detail::default()
    }
}

fn marking(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    const DESC: ListDesc = ListDesc {
        detail: marked,
        ..SERVICES
    };
    list(
        &pane(hub),
        PaneKind::Daemons,
        &DESC,
        hub.model.tables.get(DAEMONS),
    )
}

fn unmarked(_: &Win, hub: &Hub, _: &mut Cx<Win>) -> El {
    const DESC: ListDesc = ListDesc {
        detail: plain,
        ..SERVICES
    };
    list(
        &pane(hub),
        PaneKind::Daemons,
        &DESC,
        hub.model.tables.get(DAEMONS),
    )
}

/// Record 1.6.1 and 1.8: a held button presses in and springs back on release.
#[test]
fn a_detail_button_presses_in_and_springs_back() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    let [x, y, w, h] = keyed(&rt.elements(), RESTART_KEY)?.rect;
    let (cx, cy) = (x + w / 2.0, y + h / 2.0);
    rt.pointer_move(cx, cy);
    rt.pointer_down(cx, cy);
    for _ in 0..30 {
        rt.frame(1.0 / 60.0);
    }
    let held = keyed(&rt.elements(), RESTART_KEY)?.scale;
    assert!(held < 0.97, "a held button at scale {held}");
    rt.pointer_up(cx, cy);
    for _ in 0..90 {
        rt.frame(1.0 / 60.0);
    }
    let back = keyed(&rt.elements(), RESTART_KEY)?.scale;
    assert!(
        (back - 1.0).abs() < 0.01,
        "a released button at scale {back}"
    );
    Ok(())
}

/// Record 1.6.7: a command with a shortcut shows it as a keycap inside its button.
#[test]
fn a_button_shows_its_command_s_shortcut() -> Check {
    let mut rt = window(services, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    assert!(says(&rt, "⌘R"), "the Restart button has no key hint");
    Ok(())
}

/// A detail may name its main button and draw any quiet, as the mockup's Stop and Dismiss.
#[test]
fn a_detail_names_its_main_button_and_its_quiet_ones() -> Check {
    let tokens = Tokens::default();
    let mut rt = window(marking, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    let all = rt.elements();
    assert_eq!(
        keyed(&all, ATTACH_KEY)?.fill,
        colour(tokens.fg),
        "the named button is not the main one"
    );
    assert_eq!(
        keyed(&all, RESTART_KEY)?.fill.a,
        0.0,
        "a quiet button is filled"
    );
    let mut rt = window(unmarked, vec![(DAEMONS, five())]);
    press(&mut rt, &service_key("b"))?;
    let all = rt.elements();
    for key in [RESTART_KEY, ATTACH_KEY] {
        assert_eq!(
            keyed(&all, key)?.fill,
            colour(tokens.panel),
            "{key} is not plain in a detail with no main button"
        );
    }
    Ok(())
}
