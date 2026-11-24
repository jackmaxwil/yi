//! The tray above the composer: attached things as chips, and the address line.

use rune::El;
use yd_core::pane::PaneKind;
use yd_ui::overlay::boxes::tray::tray;
use yd_ui::pane::PaneCx;

/// This chat pane's tray, keyed `<pane key>.context-tray`, for above the approval card:
/// nothing while nothing is attached.
pub fn view(pane: &PaneCx<'_>) -> Option<El> {
    let key = [&pane.key(PaneKind::Chat), ".context-tray"].concat();
    tray(pane.hub, &key)
}

#[cfg(test)]
mod tests {
    use std::error::Error;

    use rune::{Cx, Env, Runtime, col};
    use yd_core::address::attach::AttachMsg;
    use yd_core::address::thing::Thing;
    use yd_core::ids::{PaneId, WindowId};
    use yd_core::msg::{Msg, step};
    use yd_ui::win::{Hub, Win};

    use super::*;

    fn pane(win: &Win, _: &mut Cx<Win>) -> El {
        let Ok(hub) = win.hub.try_borrow() else {
            return col();
        };
        let pane = PaneCx {
            hub: &hub,
            pane: PaneId(3),
            focused: true,
            edits: None,
        };
        col().key("root").w(280.0).children(view(&pane))
    }

    #[test]
    fn a_chat_pane_shows_its_tray_only_while_something_is_attached() -> Result<(), Box<dyn Error>> {
        let hub = Hub::new(&[], Vec::new());
        let win = Win::new(hub.clone(), WindowId(0));
        let mut rt = Runtime::new(win, pane, (280.0, 300.0), Env::new(true, 1.0));
        rt.frame(1.0 / 60.0);
        let tray = |rt: &Runtime<Win>| {
            let mut all = rt.elements().into_iter();
            all.find(|e| e.key.as_deref() == Some("chat-3.context-tray"))
        };
        assert!(tray(&rt).is_none(), "nothing is attached");
        let lines = Thing::at(
            &hub.try_borrow()?.model,
            "yi://lines/src/lib.rs#L4".parse()?,
        );
        step(
            &mut hub.try_borrow_mut()?.model,
            Msg::Attach(AttachMsg::Toggle(lines)),
        );
        rt.update(|_, _| {});
        rt.frame(1.0 / 60.0);
        let drawn = tray(&rt).ok_or("no tray above the composer")?;
        assert!(
            (drawn.rect[2] - 280.0).abs() < 0.01,
            "it fills the pane's width"
        );
        let says = rt.elements().into_iter().filter_map(|e| e.text);
        let says: Vec<String> = says.collect();
        assert!(says.iter().any(|said| said == "lib.rs line 4"), "{says:?}");
        assert!(says.iter().any(|said| said == "1 attached"), "{says:?}");
        Ok(())
    }
}
