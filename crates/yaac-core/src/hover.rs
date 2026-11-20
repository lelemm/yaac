//! The agents card's open/close rule (docs/design/screens.md, "The agents panel": it must not
//! flicker). The card opens when the pointer reaches the strip where the pill sits, stays open
//! while the pointer is anywhere over the card **or** that strip, and closes only after the
//! pointer has been outside both for [`CLOSE_DELAY`] seconds. Both regions are fixed by the
//! layout, never taken from the widget's own (growing and shrinking) outline.
//!
//! Pure: positions and times come in as parameters.

/// Seconds the pointer must stay outside before the card closes.
pub const CLOSE_DELAY: f64 = 0.25;

/// An axis-aligned rectangle in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Region {
    /// Left, top.
    pub min: (f32, f32),
    /// Right, bottom.
    pub max: (f32, f32),
}

impl Region {
    /// A region from its corners.
    pub fn new(min: (f32, f32), max: (f32, f32)) -> Self {
        Self { min, max }
    }

    /// Whether `p` is inside (edges included).
    pub fn contains(&self, p: (f32, f32)) -> bool {
        p.0 >= self.min.0 && p.0 <= self.max.0 && p.1 >= self.min.1 && p.1 <= self.max.1
    }
}

/// What the widget should do after a step.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HoverStep {
    /// Draw the card (otherwise the pill).
    pub open: bool,
    /// Schedule one repaint this many seconds from now (the pending close). `None`: nothing
    /// is pending, so nothing needs to wake the window.
    pub wake_in: Option<f64>,
}

/// The card's open/close state.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct HoverCard {
    open: bool,
    outside_since: Option<f64>,
}

impl HoverCard {
    /// Whether the card is open.
    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Close at once (a row was followed, the panel was docked).
    pub fn close(&mut self) {
        *self = Self::default();
    }

    /// Open at once (screenshots; keyboard focus arriving from elsewhere).
    pub fn open(&mut self) {
        self.open = true;
        self.outside_since = None;
    }

    /// One frame: the pointer (if over the window), the pill's strip, the card's region, and
    /// whether anything in the card has keyboard focus, at time `now` (seconds).
    pub fn step(&mut self, pointer: Option<(f32, f32)>, strip: Region, card: Region, focused: bool, now: f64) -> HoverStep {
        let inside = focused || pointer.is_some_and(|p| strip.contains(p) || (self.open && card.contains(p)));
        if inside {
            self.open = true;
            self.outside_since = None;
            return HoverStep { open: true, wake_in: None };
        }
        if !self.open {
            return HoverStep { open: false, wake_in: None };
        }
        let since = *self.outside_since.get_or_insert(now);
        let left = since + CLOSE_DELAY - now;
        if left <= 0.0 {
            self.close();
            HoverStep { open: false, wake_in: None }
        } else {
            HoverStep { open: true, wake_in: Some(left) }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The pill: a 44 × 180 strip at the right edge; the card: 300 × 264, sharing its right edge
    // and centred on the same line.
    fn strip() -> Region {
        Region::new((1544.0, 410.0), (1588.0, 590.0))
    }
    fn card() -> Region {
        Region::new((1288.0, 368.0), (1588.0, 632.0))
    }

    /// Replay pointer positions at times; return whether the card was open after each.
    fn replay(path: &[(Option<(f32, f32)>, f64)]) -> Vec<bool> {
        let mut h = HoverCard::default();
        path.iter().map(|(p, t)| h.step(*p, strip(), card(), false, *t).open).collect()
    }

    #[test]
    fn table() {
        let cases: &[(&str, &[(Option<(f32, f32)>, f64)], &[bool])] = &[
            ("far away: stays closed", &[(Some((100.0, 100.0)), 0.0), (Some((200.0, 500.0)), 0.1)], &[false, false]),
            (
                "over the card's area before it opened: does not open it",
                &[(Some((1300.0, 500.0)), 0.0), (Some((1400.0, 400.0)), 0.1)],
                &[false, false],
            ),
            ("reaching the strip opens", &[(Some((1560.0, 500.0)), 0.0)], &[true]),
            (
                // The owner's flicker: moving across the open card, over parts that are not the
                // pill, must never close it.
                "moving across the open card keeps it open on every frame",
                &[
                    (Some((1560.0, 500.0)), 0.00),
                    (Some((1500.0, 500.0)), 0.02),
                    (Some((1400.0, 450.0)), 0.04),
                    (Some((1300.0, 380.0)), 0.06),
                    (Some((1295.0, 620.0)), 0.08),
                    (Some((1580.0, 630.0)), 0.10),
                    (Some((1450.0, 500.0)), 2.00),
                ],
                &[true, true, true, true, true, true, true],
            ),
            (
                "leaving: stays open for the delay, then closes",
                &[(Some((1560.0, 500.0)), 0.0), (Some((900.0, 500.0)), 1.0), (Some((900.0, 500.0)), 1.1), (Some((900.0, 500.0)), 1.24), (Some((900.0, 500.0)), 1.25)],
                &[true, true, true, true, false],
            ),
            (
                "a brief excursion outside and back does not close it",
                &[(Some((1400.0, 500.0)), 0.0), (Some((1560.0, 500.0)), 0.0), (Some((1200.0, 500.0)), 0.5), (Some((1280.0, 500.0)), 0.6), (Some((1300.0, 500.0)), 0.7), (Some((1200.0, 500.0)), 0.9), (Some((1200.0, 500.0)), 1.1)],
                &[false, true, true, true, true, true, true],
            ),
            (
                "the pointer leaving the window counts as outside",
                &[(Some((1560.0, 500.0)), 0.0), (None, 0.1), (None, 0.36)],
                &[true, true, false],
            ),
            (
                "once closed, the card's area alone does not reopen it",
                &[(Some((1560.0, 500.0)), 0.0), (None, 0.1), (None, 0.4), (Some((1300.0, 500.0)), 0.5)],
                &[true, true, false, false],
            ),
        ];
        for (name, path, want) in cases {
            assert_eq!(replay(path), *want, "{name}");
        }
    }

    #[test]
    fn a_pending_close_asks_for_one_wake_and_nothing_else_does() {
        let mut h = HoverCard::default();
        assert_eq!(h.step(None, strip(), card(), false, 0.0).wake_in, None, "idle: no wake");
        assert_eq!(h.step(Some((1560.0, 500.0)), strip(), card(), false, 1.0).wake_in, None, "inside: no wake");
        let s = h.step(Some((10.0, 10.0)), strip(), card(), false, 2.0);
        assert!(s.open);
        assert!((s.wake_in.unwrap() - CLOSE_DELAY).abs() < 1e-9);
        let s = h.step(Some((10.0, 10.0)), strip(), card(), false, 2.1);
        assert!((s.wake_in.unwrap() - 0.15).abs() < 1e-9);
        let s = h.step(Some((10.0, 10.0)), strip(), card(), false, 2.25);
        assert_eq!(s, HoverStep { open: false, wake_in: None });
        assert_eq!(h.step(Some((10.0, 10.0)), strip(), card(), false, 3.0).wake_in, None, "closed: no wake");
    }

    #[test]
    fn keyboard_focus_holds_it_open() {
        let mut h = HoverCard::default();
        assert!(h.step(None, strip(), card(), true, 0.0).open);
        assert!(h.step(None, strip(), card(), true, 5.0).open);
        assert!(h.step(None, strip(), card(), false, 5.1).open);
        assert!(!h.step(None, strip(), card(), false, 5.4).open);
    }
}
