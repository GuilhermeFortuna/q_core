//! Intrabar stop and target fills: the entry check, the bar screen and the price walk.
//!
//! The screen and the walk use the same comparisons, so a bar that fails the screen cannot hold
//! a fill. A stop triggers at or beyond its level; a target triggers strictly beyond it.

use std::cmp::Ordering;

use crate::exits::Side;

use super::run::ExitReason;

/// Per-bar level columns, in absolute prices. `NaN` means no level on that bar.
#[derive(Clone, Copy, Debug)]
pub struct ProtectiveColumns<'a> {
    pub stop_price: &'a [f64],
    pub target_price: &'a [f64],
    /// Fill price of an entry decided on that bar, filled on the same bar. `NaN` means no price,
    /// so the entry queues for the next open. `None` leaves every entry queued.
    pub entry_price: Option<&'a [f64]>,
}

/// One bar's trade prices in the order they traded.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct IntrabarPrices {
    /// Wall-clock microseconds, one per price.
    pub time_us: Vec<i64>,
    pub price: Vec<f64>,
}

/// Supplies a bar's trade prices when the kernel asks for them. An `Err` ends the run.
pub trait IntrabarSource {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String>;
}

/// Entries refused by the entry check. Nothing was opened for them.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RejectedEntries {
    pub bar: Vec<i64>,
    /// `+1` long, `-1` short.
    pub side: Vec<i8>,
    pub fill_price: Vec<f64>,
    /// `NaN` = no stop.
    pub stop_price: Vec<f64>,
    /// `NaN` = no target.
    pub target_price: Vec<f64>,
}

impl RejectedEntries {
    pub(crate) fn push(&mut self, bar: usize, side: Side, fill: f64, levels: Levels) {
        self.bar.push(bar as i64);
        self.side.push(match side {
            Side::Long => 1,
            Side::Short => -1,
        });
        self.fill_price.push(fill);
        self.stop_price.push(levels.stop);
        self.target_price.push(levels.target);
    }
}

/// One trade's stop and target. Both are `NaN` where unset.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Levels {
    pub(crate) stop: f64,
    pub(crate) target: f64,
}

fn is_set(level: f64) -> bool {
    !level.is_nan()
}

impl Levels {
    pub(crate) const NONE: Self = Self {
        stop: f64::NAN,
        target: f64::NAN,
    };

    /// The levels an entry queued on `bar` carries.
    pub(crate) fn queued_on(protective: Option<ProtectiveColumns<'_>>, bar: usize) -> Self {
        protective.map_or(Self::NONE, |columns| Self {
            stop: columns.stop_price[bar],
            target: columns.target_price[bar],
        })
    }

    /// Entry check. A long needs its stop below and its target above the fill; a short needs the
    /// reverse. Only the levels that are set are checked.
    pub(crate) fn rejects(self, side: Side, fill: f64) -> bool {
        match side {
            Side::Long => {
                (is_set(self.stop) && self.stop >= fill)
                    || (is_set(self.target) && self.target <= fill)
            }
            Side::Short => {
                (is_set(self.stop) && self.stop <= fill)
                    || (is_set(self.target) && self.target >= fill)
            }
        }
    }

    /// Screen: whether the bar's range reaches a set level.
    pub(crate) fn reached_by_range(self, side: Side, high: f64, low: f64) -> bool {
        match side {
            Side::Long => {
                (is_set(self.stop) && low <= self.stop)
                    || (is_set(self.target) && high > self.target)
            }
            Side::Short => {
                (is_set(self.stop) && high >= self.stop)
                    || (is_set(self.target) && low < self.target)
            }
        }
    }

    /// The exit a single trade price reaches, with its fill price. A stop fills at the price
    /// at or beyond it; a target fills at its level once a price trades strictly through it.
    pub(crate) fn trigger(self, side: Side, price: f64) -> Option<(ExitReason, f64)> {
        let (stopped, targeted) = match side {
            Side::Long => (
                is_set(self.stop) && price <= self.stop,
                is_set(self.target) && price > self.target,
            ),
            Side::Short => (
                is_set(self.stop) && price >= self.stop,
                is_set(self.target) && price < self.target,
            ),
        };
        if stopped {
            Some((ExitReason::StopLoss, price))
        } else if targeted {
            Some((ExitReason::TakeProfit, self.target))
        } else {
            None
        }
    }
}

/// Index of the trade price that touches a priced entry, or `None` when no price reaches it.
///
/// A bar that opens above the price trades down to it, so the touch is the first price at or
/// below it; a bar that opens below trades up to it, so the touch is the first price at or above
/// it. A bar that opens exactly at the price touches on its first print. A `NaN` open has no side.
pub(crate) fn touch_index(prices: &[f64], price: f64, open: f64) -> Option<usize> {
    match open.partial_cmp(&price)? {
        Ordering::Greater => prices.iter().position(|&trade| trade <= price),
        Ordering::Less => prices.iter().position(|&trade| trade >= price),
        Ordering::Equal => (!prices.is_empty()).then_some(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn long(stop: f64, target: f64) -> Levels {
        Levels { stop, target }
    }

    #[test]
    fn a_bar_opening_at_the_price_touches_on_its_first_print() {
        assert_eq!(touch_index(&[100.0, 101.5], 100.0, 100.0), Some(0));
        assert_eq!(touch_index(&[], 100.0, 100.0), None);
    }

    #[test]
    fn a_bar_opening_below_touches_at_the_first_print_at_or_above_the_price() {
        // Equal to the price touches on that print; one step short does not.
        assert_eq!(touch_index(&[99.0, 101.0, 100.0], 101.0, 99.0), Some(1));
        assert_eq!(touch_index(&[99.0, 100.5, 101.0], 101.0, 99.0), Some(2));
        assert_eq!(touch_index(&[99.0, 100.9], 101.0, 99.0), None);
    }

    #[test]
    fn a_bar_opening_above_touches_at_the_first_print_at_or_below_the_price() {
        assert_eq!(touch_index(&[101.0, 99.0, 98.0], 99.0, 101.0), Some(1));
        assert_eq!(touch_index(&[101.0, 99.5, 99.0], 99.0, 101.0), Some(2));
        assert_eq!(touch_index(&[101.0, 99.1], 99.0, 101.0), None);
    }

    #[test]
    fn a_long_stop_is_reached_at_the_level_and_a_target_only_beyond_it() {
        let levels = long(95.0, 110.0);
        assert!(levels.reached_by_range(Side::Long, 100.0, 95.0));
        assert!(!levels.reached_by_range(Side::Long, 110.0, 96.0));
        assert!(levels.reached_by_range(Side::Long, 110.5, 96.0));
    }

    #[test]
    fn a_short_stop_is_reached_at_the_level_and_a_target_only_beyond_it() {
        let levels = Levels {
            stop: 105.0,
            target: 90.0,
        };
        assert!(levels.reached_by_range(Side::Short, 105.0, 100.0));
        assert!(!levels.reached_by_range(Side::Short, 104.0, 90.0));
        assert!(levels.reached_by_range(Side::Short, 104.0, 89.5));
    }

    #[test]
    fn a_stop_fills_at_the_first_price_at_or_beyond_it() {
        assert_eq!(
            long(95.0, f64::NAN).trigger(Side::Long, 95.0),
            Some((ExitReason::StopLoss, 95.0))
        );
        assert_eq!(
            long(95.0, f64::NAN).trigger(Side::Long, 94.0),
            Some((ExitReason::StopLoss, 94.0))
        );
        assert_eq!(long(95.0, f64::NAN).trigger(Side::Long, 96.0), None);
    }

    #[test]
    fn a_target_fills_at_its_level_only_when_a_price_trades_strictly_beyond_it() {
        assert_eq!(long(f64::NAN, 110.0).trigger(Side::Long, 110.0), None);
        assert_eq!(
            long(f64::NAN, 110.0).trigger(Side::Long, 111.0),
            Some((ExitReason::TakeProfit, 110.0))
        );
    }

    #[test]
    fn a_short_stop_and_target_mirror_the_long_comparisons() {
        let levels = Levels {
            stop: 105.0,
            target: 90.0,
        };
        assert_eq!(
            levels.trigger(Side::Short, 105.0),
            Some((ExitReason::StopLoss, 105.0))
        );
        assert_eq!(levels.trigger(Side::Short, 90.0), None);
        assert_eq!(
            levels.trigger(Side::Short, 89.0),
            Some((ExitReason::TakeProfit, 90.0))
        );
    }

    #[test]
    fn the_entry_check_flags_only_levels_that_are_set_on_the_wrong_side() {
        assert!(long(100.0, f64::NAN).rejects(Side::Long, 100.0));
        assert!(long(f64::NAN, 100.0).rejects(Side::Long, 100.0));
        assert!(!long(99.0, 101.0).rejects(Side::Long, 100.0));
        assert!(!long(f64::NAN, f64::NAN).rejects(Side::Long, 100.0));
        assert!(Levels {
            stop: 99.0,
            target: f64::NAN
        }
        .rejects(Side::Short, 100.0));
        assert!(!Levels {
            stop: 101.0,
            target: f64::NAN
        }
        .rejects(Side::Short, 100.0));
    }
}
