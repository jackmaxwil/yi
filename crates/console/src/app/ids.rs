//! Invariant: kitty ids by owner, none shared: pane cards 7200-7399, solo orb 7601-7602, logos 7610
//! on, question 7690-7691, notebook 7701-7702, pane orbs 7800-7999, pool 8000 on, browser 0xF00000.

use std::collections::HashMap;

use super::App;
use crate::layout::PaneId;
use crate::model::{PaneContent, SessionId};

const SLOTS: u32 = 100;
const ORB_ID_BASE: u32 = 7800;
const CARD_ID_BASE: u32 = 7200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Slot(u32);

pub(crate) type Owners = HashMap<PaneId, (Option<SessionId>, Option<Slot>)>;

pub(crate) fn orb_ids(slot: Slot) -> [u32; 2] {
    pair(ORB_ID_BASE, slot)
}

/// Incident: one id pair for every pane let a hidden pane's card delete the focused pane's.
pub(crate) fn card_ids(slot: Slot) -> [u32; 2] {
    pair(CARD_ID_BASE, slot)
}

fn pair(base: u32, slot: Slot) -> [u32; 2] {
    let first = base.saturating_add(slot.0.saturating_mul(2));
    [first, first.saturating_add(1)]
}

fn pairs(slot: Slot) -> impl Iterator<Item = u32> {
    orb_ids(slot).into_iter().chain(card_ids(slot))
}

/// Incident: ids from the never-reused pane id reached the pool's 8000 at pane 100, so a close
/// deleted a live avatar. A gone chat frees its slot; a pane past the hundredth draws text.
pub(crate) fn lease(app: &mut App, owners: &mut Owners, out: &mut impl std::io::Write) {
    owners.retain(|pane, (owner, slot)| {
        let kept = app
            .state
            .panes
            .get(pane)
            .is_some_and(|pane| match &pane.content {
                PaneContent::Session {
                    session,
                    chat: Some(_),
                } => session == owner,
                _ => false,
            });
        for id in slot.filter(|_| !kept).into_iter().flat_map(pairs) {
            let _ = yi_tui::orb::kitty::delete_id(out, id);
        }
        kept
    });
    let mut held: Vec<Slot> = owners.values().filter_map(|(_, slot)| *slot).collect();
    for (id, pane) in &mut app.state.panes {
        let PaneContent::Session {
            session,
            chat: Some(chat),
        } = &mut pane.content
        else {
            continue;
        };
        let first = !owners.contains_key(id);
        let (_, slot) = owners.entry(*id).or_insert_with(|| (session.clone(), None));
        if slot.is_none() {
            *slot = (0..SLOTS).map(Slot).find(|free| !held.contains(free));
            held.extend(*slot);
        }
        if chat.slot != *slot {
            chat.orb = slot.map_or_else(Default::default, |slot| {
                yi_tui::orb::Tick::with_ids(orb_ids(slot))
            });
            chat.card = slot.map_or_else(Default::default, |slot| {
                yi_tui::question::CardTick::with_ids(card_ids(slot))
            });
            chat.slot = *slot;
        }
        if first && slot.is_none() {
            let full = format!("[…] {SLOTS} of {SLOTS} image slots in use");
            chat.app.notice(format!(
                "{full} · this pane's mark is text until a pane closes"
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use ratatui::layout::Direction;

    use crate::app::App;
    use crate::model::SessionId;

    type Shot = Result<(String, Vec<String>), Box<dyn std::error::Error>>;

    fn ids(out: &str, verb: &str) -> Vec<u32> {
        out.match_indices(verb)
            .filter_map(|(at, _)| out.get(at.saturating_add(verb.len())..))
            .filter_map(|rest| rest.strip_prefix("i="))
            .filter_map(|rest| rest.split(|c: char| !c.is_ascii_digit()).next())
            .filter_map(|digits| digits.parse().ok())
            .collect()
    }

    fn chat_range(id: &u32) -> bool {
        (7200..7400).contains(id) || (7800..8000).contains(id)
    }

    fn open(app: &mut App, split: bool, n: usize) -> Result<(), Box<dyn std::error::Error>> {
        if split {
            app.state.split_focused(Direction::Horizontal);
        } else {
            app.state.new_tab();
        }
        let pane = app.state.focused_pane_id().ok_or("no pane")?;
        app.bind_pane(pane, &SessionId(format!("s-{n}")));
        Ok(())
    }

    fn chats(app: &App) -> usize {
        let bound = app
            .state
            .panes
            .values()
            .filter(|pane| pane.session().is_some());
        bound.count()
    }

    /// Dies with the hundredth pane's orb sent on the avatar pool's ids, a 101st live pane
    /// drawing an image or not saying why, or a closed pane's slot left unused.
    #[test]
    fn the_hundredth_pane_draws_below_the_avatar_ids_and_the_next_waits_for_a_close()
    -> Result<(), Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        app.kitty = true;
        let mut terminal = ratatui::Terminal::new(ratatui::backend::TestBackend::new(120, 30))?;
        let mut owners = super::Owners::default();
        let mut present = |app: &mut App| -> Shot {
            let mut out = Vec::new();
            app.dirty = true;
            crate::present(app, &mut terminal, &theme, &mut owners, &mut out)?;
            let buffer = terminal.backend().buffer();
            let rows = buffer.content.chunks(usize::from(buffer.area.width));
            let rows = rows.map(|row| row.iter().map(|cell| cell.symbol()).collect());
            Ok((String::from_utf8_lossy(&out).into_owned(), rows.collect()))
        };
        let (first, _) = present(&mut app)?;
        let pool = ids(&first, "a=t,");
        let home = app.state.focused_pane_id().ok_or("no pane")?;
        app.bind_pane(home, &SessionId("s-0".to_owned()));
        while chats(&app) < 100 {
            let live = chats(&app);
            open(&mut app, live % 2 == 1, live)?;
        }
        let (out, _) = present(&mut app)?;
        let sent = ids(&out, "a=t,");
        let orb: Vec<u32> = sent.iter().copied().filter(chat_range).collect();
        assert_eq!(orb.len(), 2, "the last tab's two orbs went out on {sent:?}");
        let mut once = sent.clone();
        once.sort_unstable();
        once.dedup();
        assert_eq!(
            once.len(),
            sent.len(),
            "one id sent for two images: {sent:?}"
        );
        assert!(
            !sent.iter().any(|id| pool.contains(id)),
            "the pool's ids were resent: {sent:?}"
        );
        open(&mut app, true, 100)?;
        let extra = app.state.focused_pane_id().ok_or("no pane")?;
        app.state.tab_mut().ok_or("no tab")?.zoomed = true;
        let (out, rows) = present(&mut app)?;
        let orb: Vec<u32> = ids(&out, "a=t,").into_iter().filter(chat_range).collect();
        assert!(orb.is_empty(), "the 101st pane sent an orb on {orb:?}");
        let said = |rows: &[String]| {
            rows.iter()
                .filter(|row| row.contains("100 of 100 image slots in use"))
                .count()
        };
        assert_eq!(
            said(&rows),
            1,
            "the 101st pane does not say why it draws text: {rows:#?}"
        );
        let (_, rows) = present(&mut app)?;
        assert_eq!(said(&rows), 1, "the 101st pane said it twice: {rows:#?}");
        let tab = app.state.tab_mut().ok_or("no tab")?;
        tab.zoomed = false;
        let sibling = tab.layout.pane_ids().into_iter().find(|id| *id != extra);
        tab.layout.focus_pane(sibling.ok_or("alone")?);
        app.state.close_focused_pane();
        if let Some(tab) = app.state.tab_mut() {
            tab.layout.focus_pane(extra);
        }
        let (out, _) = present(&mut app)?;
        let deleted = ids(&out, "a=d,d=I,");
        let sent: Vec<u32> = ids(&out, "a=t,").into_iter().filter(chat_range).collect();
        assert!(
            sent.iter().filter(|id| deleted.contains(id)).count() == 1,
            "the 101st pane did not take the closed pane's slot: sent {sent:?}, deleted {deleted:?}"
        );
        app.bind_pane(extra, &SessionId("s-away".to_owned()));
        present(&mut app)?;
        app.bind_pane(extra, &SessionId("s-100".to_owned()));
        let (out, _) = present(&mut app)?;
        let back: Vec<u32> = ids(&out, "a=t,")
            .into_iter()
            .filter(|id| *id < 8000)
            .collect();
        assert!(
            back.iter().all(chat_range) && !back.is_empty(),
            "a parked chat came back on {back:?}"
        );
        Ok(())
    }

    fn slots(app: &App) -> Vec<(crate::layout::PaneId, Option<super::Slot>)> {
        let chats = app
            .state
            .panes
            .iter()
            .filter_map(|(id, pane)| match &pane.content {
                crate::model::PaneContent::Session {
                    chat: Some(chat), ..
                } => Some((*id, chat.slot)),
                _ => None,
            });
        chats.collect()
    }

    /// Dies with a slot leaked by a close, a rebind or a park, a slot held twice, or an id that
    /// reaches the pool's 8000 or the notebook's 7701 over 250 opens.
    #[test]
    fn every_close_frees_its_slot_across_250_opens() -> Result<(), Box<dyn std::error::Error>> {
        let theme = yi_tui::colors::Theme::new(yi_tui::colors::ColorTier::TrueColor, true);
        let mut app = App::new("/r".to_owned(), theme);
        let mut owners = super::Owners::default();
        let mut lease = |app: &mut App| {
            let mut out = Vec::new();
            super::lease(app, &mut owners, &mut out);
            let out = String::from_utf8_lossy(&out).into_owned();
            let mut deleted = ids(&out, "a=d,d=I,");
            deleted.sort_unstable();
            let held = slots(app);
            let mut taken: Vec<u32> = held
                .iter()
                .filter_map(|(_, slot)| *slot)
                .flat_map(super::pairs)
                .collect();
            taken.sort_unstable();
            let count = taken.len();
            taken.dedup();
            assert_eq!(taken.len(), count, "a slot is held twice");
            assert!(taken.iter().all(chat_range), "{taken:?}");
            assert_eq!(
                count / 4,
                held.len().min(100),
                "a live pane went without a free slot"
            );
            (deleted, held)
        };
        let home = app.state.focused_pane_id().ok_or("no pane")?;
        app.bind_pane(home, &SessionId("s-0".to_owned()));
        let mut opens = 1_usize;
        while opens <= 100 {
            open(&mut app, opens % 2 == 1, opens)?;
            opens = opens.saturating_add(1);
            lease(&mut app);
        }
        let (_, held) = lease(&mut app);
        let extra = app.state.focused_pane_id().ok_or("no pane")?;
        assert_eq!(
            held.iter()
                .find(|(id, _)| *id == extra)
                .map(|(_, slot)| *slot),
            Some(None)
        );
        let (mut turn, mut parked, mut restored) = (0_usize, None, 0_usize);
        while opens < 250 {
            let tabs = app.state.tabs.len();
            app.state.select_tab(turn.checked_rem(tabs).unwrap_or(0));
            turn = turn.saturating_add(7);
            let focused = app.state.focused_pane_id().ok_or("no pane")?;
            let before = slots(&app)
                .into_iter()
                .find(|(id, _)| *id == focused)
                .and_then(|(_, slot)| slot);
            let rebind = turn.checked_rem(5) == Some(0);
            if rebind {
                let back = parked
                    .take()
                    .unwrap_or_else(|| SessionId(format!("s-x{turn}")));
                parked = app
                    .state
                    .panes
                    .get(&focused)
                    .and_then(|pane| pane.session().cloned());
                restored =
                    restored.saturating_add(usize::from(app.state.parked.contains_key(&back)));
                app.bind_pane(focused, &back);
            } else if app.state.close_focused_pane().is_none() {
                continue;
            }
            let (deleted, _) = lease(&mut app);
            let mut freed = before
                .into_iter()
                .flat_map(super::pairs)
                .collect::<Vec<_>>();
            freed.sort_unstable();
            assert_eq!(deleted, freed, "turn {turn} deleted another pane's ids");
            if !rebind {
                open(&mut app, true, opens)?;
                opens = opens.saturating_add(1);
                lease(&mut app);
            }
        }
        let (_, held) = lease(&mut app);
        assert!(restored > 0, "no parked chat came back");
        assert_eq!(held.len(), 101);
        assert_eq!(held.iter().filter(|(_, slot)| slot.is_none()).count(), 1);
        Ok(())
    }
}
