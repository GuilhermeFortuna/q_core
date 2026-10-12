//! Tick-ordered breakeven activation and its pre-activation emergency stop.

use super::{ExitParams, Side};

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct BreakevenStop {
    armed: bool,
}

impl BreakevenStop {
    /// Replay activation even when the stop is out of range, to retain state for later bars.
    pub(crate) fn reached_by_range(
        self,
        side: Side,
        entry: f64,
        high: f64,
        low: f64,
        params: &ExitParams,
    ) -> bool {
        if !params.intrabar_breakeven() {
            return false;
        }
        let favorable = match side {
            Side::Long => high,
            Side::Short => low,
        };
        let adverse = match side {
            Side::Long => low,
            Side::Short => high,
        };
        (!self.armed && gain(side, entry, favorable) >= params.breakeven_trigger_pct)
            || self.stopped(side, entry, adverse, params)
    }

    /// Returns whether a stop fires at this price and whether it is the emergency stop.
    pub(crate) fn on_price(
        &mut self,
        side: Side,
        entry: f64,
        price: f64,
        params: &ExitParams,
    ) -> Option<bool> {
        if !params.intrabar_breakeven() {
            return None;
        }
        if !self.armed && gain(side, entry, price) >= params.breakeven_trigger_pct {
            self.armed = true;
        }
        self.stopped(side, entry, price, params)
            .then_some(!self.armed)
    }

    #[allow(
        clippy::suboptimal_flops,
        reason = "preserve established breakeven percentage arithmetic"
    )]
    fn stopped(self, side: Side, entry: f64, price: f64, params: &ExitParams) -> bool {
        let distance = params.breakeven_emergency_stop_ticks.unwrap_or(0) as f64 * params.tick_size;
        if !self.armed && distance <= 0.0 {
            return false;
        }
        match side {
            Side::Long => {
                price
                    <= if self.armed {
                        entry * (1.0 + params.breakeven_offset_pct)
                    } else {
                        entry - distance
                    }
            }
            Side::Short => {
                price
                    >= if self.armed {
                        entry * (1.0 - params.breakeven_offset_pct)
                    } else {
                        entry + distance
                    }
            }
        }
    }
}

#[allow(
    clippy::suboptimal_flops,
    reason = "preserve established breakeven gain arithmetic"
)]
fn gain(side: Side, entry: f64, price: f64) -> f64 {
    match side {
        Side::Long => (price - entry) / entry,
        Side::Short => (entry - price) / entry,
    }
}
