//! Intrabar stop and target fills: the entry check, the bar screen and the price walk.
//!
//! The screen and the walk use the same comparisons, so a bar that fails the screen cannot hold
//! a fill. A stop triggers at or beyond its level; a target triggers strictly beyond it.

use crate::exits::Side;

use super::run::ExitReason;

/// Per-bar level columns, in absolute prices. `NaN` means no level on that bar.
#[derive(Clone, Copy, Debug)]
pub struct ProtectiveColumns<'a> {
    pub stop_price: &'a [f64],
    pub target_price: &'a [f64],
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

/// A level that a bar's prices reached: the exit reason, its fill price and the time of the
/// price that reached it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Trigger {
    pub(crate) reason: ExitReason,
    pub(crate) price: f64,
    pub(crate) time_us: i64,
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

    /// Walks `prices` in order and returns the first price that reaches a level. The first price
    /// is skipped for a trade opened on this bar, since it is that trade's own fill.
    pub(crate) fn first_trigger(
        self,
        side: Side,
        prices: &IntrabarPrices,
        skip_first: bool,
    ) -> Option<Trigger> {
        let points = prices.time_us.iter().zip(&prices.price);
        for (&time_us, &price) in points.skip(usize::from(skip_first)) {
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
                return Some(Trigger {
                    reason: ExitReason::StopLoss,
                    price,
                    time_us,
                });
            }
            if targeted {
                return Some(Trigger {
                    reason: ExitReason::TakeProfit,
                    price: self.target,
                    time_us,
                });
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prices(points: &[(i64, f64)]) -> IntrabarPrices {
        IntrabarPrices {
            time_us: points.iter().map(|point| point.0).collect(),
            price: points.iter().map(|point| point.1).collect(),
        }
    }

    fn long(stop: f64, target: f64) -> Levels {
        Levels { stop, target }
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
    fn the_walk_stops_at_a_price_equal_to_the_stop_and_not_at_one_equal_to_the_target() {
        let points = prices(&[(1, 96.0), (2, 95.0), (3, 94.0)]);
        let hit = long(95.0, f64::NAN)
            .first_trigger(Side::Long, &points, false)
            .unwrap();
        assert_eq!(hit.price.to_bits(), 95.0_f64.to_bits());
        assert_eq!(hit.time_us, 2);

        let points = prices(&[(1, 110.0), (2, 111.0)]);
        let hit = long(f64::NAN, 110.0)
            .first_trigger(Side::Long, &points, false)
            .unwrap();
        assert_eq!(hit.reason, ExitReason::TakeProfit);
        assert_eq!(hit.price.to_bits(), 110.0_f64.to_bits());
        assert_eq!(hit.time_us, 2);
    }

    #[test]
    fn the_entry_bar_skip_ignores_only_the_first_price() {
        let points = prices(&[(1, 99.0), (2, 99.5)]);
        assert_eq!(
            long(99.0, f64::NAN).first_trigger(Side::Long, &points, true),
            None
        );
        assert!(long(99.0, f64::NAN)
            .first_trigger(Side::Long, &points, false)
            .is_some());
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
