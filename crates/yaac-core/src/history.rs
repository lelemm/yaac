//! Navigation history: back and forward, like a browser (decisions D87, D88).
//!
//! Every place the user goes — a thread, Settings, a new-thread draft — is an entry. Each
//! entry remembers where the user was reading (an item and an offset within it), so going
//! back to a long transcript lands on the same lines even after new items arrived. Going
//! somewhere new after going back drops the forward entries. A thread deleted since is
//! skipped. Re-opening the place already open adds nothing. At most [`CAP`] entries are kept.
//!
//! Pure: whether a place still exists is asked of the caller.

use yaac_protocol::ItemId;

use crate::key::ThreadKey;
use crate::settings::Page;

/// The most entries kept; the oldest go first.
pub const CAP: usize = 100;

/// How many entries a button's menu lists.
pub const MENU_MAX: usize = 10;

/// A place in the window.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Place {
    /// A thread.
    Thread(ThreadKey),
    /// Settings, at a page.
    Settings(Page),
    /// A new-thread draft.
    Draft(u64),
}

impl Place {
    /// The same place for history purposes (Settings is one place whatever its page).
    fn same(&self, other: &Place) -> bool {
        match (self, other) {
            (Place::Settings(_), Place::Settings(_)) => true,
            _ => self == other,
        }
    }
}

/// Where the user was reading: an item, and how far below its top the view began.
#[derive(Clone, Debug, PartialEq)]
pub struct Anchor {
    /// The item at the top of the view.
    pub item: ItemId,
    /// Points from the item's top to the top of the view.
    pub offset: f32,
}

/// One entry.
#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    /// The place.
    pub place: Place,
    /// Where the user was reading, once known.
    pub anchor: Option<Anchor>,
}

/// The history.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct History {
    entries: Vec<Entry>,
    cur: Option<usize>,
}

impl History {
    /// The current entry.
    pub fn current(&self) -> Option<&Entry> {
        self.cur.and_then(|i| self.entries.get(i))
    }

    /// The user went to `place` (a click, a link, a shortcut). `anchor` is where a link
    /// landed (a particular message), if anywhere.
    pub fn visit(&mut self, place: Place, anchor: Option<Anchor>) {
        if let Some(c) = self.cur
            && self.entries[c].place.same(&place)
        {
            // Re-selecting what is open: no new entry. A link to a message here still lands.
            let e = &mut self.entries[c];
            e.place = place;
            if anchor.is_some() {
                e.anchor = anchor;
            }
            return;
        }
        let keep = self.cur.map_or(0, |c| c + 1);
        self.entries.truncate(keep);
        self.entries.push(Entry { place, anchor });
        if self.entries.len() > CAP {
            let drop = self.entries.len() - CAP;
            self.entries.drain(..drop);
        }
        self.cur = Some(self.entries.len() - 1);
    }

    /// Remember where the user is reading in the current entry.
    pub fn set_anchor(&mut self, anchor: Option<Anchor>) {
        if let Some(c) = self.cur {
            self.entries[c].anchor = anchor;
        }
    }

    /// The indices of the existing entries in one direction, nearest first.
    fn steps(&self, forward: bool, exists: &dyn Fn(&Place) -> bool) -> Vec<usize> {
        let Some(c) = self.cur else { return Vec::new() };
        let idx: Box<dyn Iterator<Item = usize>> = if forward { Box::new(c + 1..self.entries.len()) } else { Box::new((0..c).rev()) };
        idx.filter(|&i| exists(&self.entries[i].place)).collect()
    }

    /// Whether Back has somewhere to go.
    pub fn can_back(&self, exists: &dyn Fn(&Place) -> bool) -> bool {
        !self.steps(false, exists).is_empty()
    }

    /// Whether Forward has somewhere to go.
    pub fn can_forward(&self, exists: &dyn Fn(&Place) -> bool) -> bool {
        !self.steps(true, exists).is_empty()
    }

    /// Move `n` places: negative is back, positive forward. Deleted places are skipped and
    /// not counted. Returns the entry arrived at, or `None` (and no move) when there is not
    /// that far to go.
    pub fn jump(&mut self, n: isize, exists: &dyn Fn(&Place) -> bool) -> Option<&Entry> {
        if n == 0 {
            return None;
        }
        let steps = self.steps(n > 0, exists);
        let i = *steps.get(n.unsigned_abs() - 1)?;
        self.cur = Some(i);
        self.entries.get(i)
    }

    /// One step back.
    pub fn back(&mut self, exists: &dyn Fn(&Place) -> bool) -> Option<&Entry> {
        self.jump(-1, exists)
    }

    /// One step forward.
    pub fn forward(&mut self, exists: &dyn Fn(&Place) -> bool) -> Option<&Entry> {
        self.jump(1, exists)
    }

    /// A button's menu: the places in that direction, nearest first, at most
    /// [`MENU_MAX`], each with the `jump` that reaches it.
    pub fn menu(&self, forward: bool, exists: &dyn Fn(&Place) -> bool) -> Vec<(isize, &Entry)> {
        let sign = if forward { 1 } else { -1 };
        self.steps(forward, exists)
            .into_iter()
            .take(MENU_MAX)
            .enumerate()
            .map(|(k, i)| (sign * isize::try_from(k + 1).unwrap_or(isize::MAX), &self.entries[i]))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use yaac_protocol::{TargetId, ThreadId};

    use super::*;

    fn t(n: &str) -> Place {
        Place::Thread(ThreadKey::new(TargetId::new("local"), ThreadId::new(n)))
    }

    fn all(_: &Place) -> bool {
        true
    }

    fn at(h: &History) -> Place {
        h.current().unwrap().place.clone()
    }

    #[test]
    fn rules_table() {
        // (name, steps, expected current, can_back, can_forward)
        enum S {
            Visit(&'static str),
            Settings,
            Draft(u64),
            Back,
            Fwd,
        }
        use S::*;
        let cases: &[(&str, &[S], Place, bool, bool)] = &[
            ("one visit: nowhere to go", &[Visit("a")], t("a"), false, false),
            ("visits push; back returns", &[Visit("a"), Visit("b"), Back], t("a"), false, true),
            ("forward after back", &[Visit("a"), Visit("b"), Back, Fwd], t("b"), true, false),
            ("new place after back drops forward", &[Visit("a"), Visit("b"), Visit("c"), Back, Back, Visit("d")], t("d"), true, false),
            ("re-selecting the open thread adds nothing", &[Visit("a"), Visit("a"), Visit("a"), Back], t("a"), false, false),
            ("settings is a place; back returns to the thread", &[Visit("a"), Settings, Back], t("a"), false, true),
            ("a draft is a place", &[Visit("a"), Draft(7), Visit("b"), Back], Place::Draft(7), true, true),
            ("back with nowhere to go does nothing", &[Visit("a"), Back, Back], t("a"), false, false),
        ];
        for (name, steps, want, b, f) in cases {
            let mut h = History::default();
            for s in *steps {
                match s {
                    Visit(n) => h.visit(t(n), None),
                    Settings => h.visit(Place::Settings(Page::Targets), None),
                    Draft(d) => h.visit(Place::Draft(*d), None),
                    Back => {
                        h.back(&all);
                    }
                    Fwd => {
                        h.forward(&all);
                    }
                }
            }
            assert_eq!(at(&h), *want, "{name}");
            assert_eq!((h.can_back(&all), h.can_forward(&all)), (*b, *f), "{name}");
        }
    }

    #[test]
    fn entries_remember_where_the_user_was_reading() {
        let mut h = History::default();
        h.visit(t("long"), None);
        h.set_anchor(Some(Anchor { item: "i40".into(), offset: 12.5 }));
        // A link lands on a particular message.
        h.visit(t("other"), Some(Anchor { item: "m3".into(), offset: 0.0 }));
        let e = h.back(&all).unwrap();
        assert_eq!(e.anchor, Some(Anchor { item: "i40".into(), offset: 12.5 }));
        let e = h.forward(&all).unwrap();
        assert_eq!(e.anchor, Some(Anchor { item: "m3".into(), offset: 0.0 }), "the link lands there again");
    }

    #[test]
    fn a_deleted_thread_is_skipped() {
        let mut h = History::default();
        for n in ["a", "gone", "b"] {
            h.visit(t(n), None);
        }
        let exists = |p: &Place| *p != t("gone");
        assert_eq!(h.back(&exists).unwrap().place, t("a"));
        assert!(!h.can_back(&exists));
        assert_eq!(h.forward(&exists).unwrap().place, t("b"));
        assert_eq!(h.menu(false, &exists).iter().map(|(n, e)| (*n, e.place.clone())).collect::<Vec<_>>(), [(-1, t("a"))]);
    }

    #[test]
    fn capped_at_a_hundred() {
        let mut h = History::default();
        for i in 0..150 {
            h.visit(t(&format!("t{i}")), None);
        }
        let mut n = 0;
        while h.back(&all).is_some() {
            n += 1;
        }
        assert_eq!(n, CAP - 1);
        assert_eq!(at(&h), t("t50"));
    }

    #[test]
    fn jumps_equal_single_presses_and_menus_list_nearest_first() {
        let mut a = History::default();
        let mut b = History::default();
        for n in ["p0", "p1", "p2", "p3", "p4", "p5"] {
            a.visit(t(n), None);
            b.visit(t(n), None);
        }
        // Back three then forward one, by jumps and by presses.
        a.jump(-3, &all);
        a.jump(1, &all);
        for _ in 0..3 {
            b.back(&all);
        }
        b.forward(&all);
        assert_eq!(a, b);
        assert_eq!(at(&a), t("p3"));
        // The entries skipped over stay on the other side.
        assert_eq!(a.menu(true, &all).iter().map(|(n, e)| (*n, e.place.clone())).collect::<Vec<_>>(), [(1, t("p4")), (2, t("p5"))]);
        assert_eq!(a.menu(false, &all).iter().map(|(n, e)| (*n, e.place.clone())).collect::<Vec<_>>(), [(-1, t("p2")), (-2, t("p1")), (-3, t("p0"))]);
        // Too far: no move.
        assert!(a.jump(5, &all).is_none());
        assert_eq!(at(&a), t("p3"));
        // A menu lists at most ten.
        let mut c = History::default();
        for i in 0..20 {
            c.visit(t(&format!("q{i}")), None);
        }
        assert_eq!(c.menu(false, &all).len(), MENU_MAX);
        assert!(c.menu(true, &all).is_empty(), "nowhere forward: no menu");
    }
}
