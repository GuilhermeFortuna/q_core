//! Streaming study states for live chart indicators.
//!
//! Exposes incremental state machines for SMA, EMA, Bollinger Bands, RSI, ATR,
//! and Session VWAP. Each state commits completed bars, previews the forming bar
//! without mutating internal state, and is bitwise identical to the batch kernels.

use crate::error::IndicatorError;
use crate::ieee::{com_from_alpha_period, com_from_span};
use crate::indicators::nan_skipping_max3;
use crate::window::{EwmMeanState, RollingMeanState, RollingVarState};

/// Streaming state for Simple Moving Average.
#[derive(Debug, Clone)]
pub struct SmaState {
    state: RollingMeanState,
}

impl SmaState {
    /// Create a new SMA state with the given period.
    pub fn new(period: i64) -> Result<Self, IndicatorError> {
        if period < 1 {
            return Err(IndicatorError::InvalidParameter {
                function: "sma",
                parameter: "period",
                value: period,
                requirement: ">= 1",
            });
        }
        let w = period as usize;
        Ok(Self {
            state: RollingMeanState::new(w, w),
        })
    }

    /// Commit one completed bar value and advance state.
    pub fn commit(&mut self, val: f64) -> f64 {
        self.state.push(val)
    }

    /// Preview the forming bar value without mutating state.
    pub fn preview(&self, val: f64) -> f64 {
        let mut clone = self.clone();
        clone.commit(val)
    }

    /// Reset internal state to initial empty conditions.
    pub fn reset(&mut self) {
        self.state.reset();
    }
}

/// Streaming state for Exponential Moving Average.
#[derive(Debug, Clone)]
pub struct EmaState {
    state: EwmMeanState,
}

impl EmaState {
    /// Create a new EMA state with the given period.
    pub fn new(period: i64) -> Result<Self, IndicatorError> {
        if period < 1 {
            return Err(IndicatorError::InvalidParameter {
                function: "ema",
                parameter: "period",
                value: period,
                requirement: ">= 1",
            });
        }
        Ok(Self {
            state: EwmMeanState::new(com_from_span(period), 0),
        })
    }

    /// Commit one completed bar value and advance state.
    pub fn commit(&mut self, val: f64) -> f64 {
        self.state.push(val)
    }

    /// Preview the forming bar value without mutating state.
    pub fn preview(&self, val: f64) -> f64 {
        let mut clone = self.clone();
        clone.commit(val)
    }

    /// Reset internal state to initial empty conditions.
    pub fn reset(&mut self) {
        self.state.reset();
    }
}

/// Output triple for Bollinger Bands at a single bar.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BollingerOutput {
    pub upper: f64,
    pub middle: f64,
    pub lower: f64,
}

/// Streaming state for Bollinger Bands.
#[derive(Debug, Clone)]
pub struct BollingerState {
    mean_state: RollingMeanState,
    var_state: RollingVarState,
    num_std: f64,
}

impl BollingerState {
    /// Create a new Bollinger Bands state with the given period and standard deviation multiplier.
    pub fn new(period: i64, num_std: f64) -> Result<Self, IndicatorError> {
        if period < 0 {
            return Err(IndicatorError::InvalidParameter {
                function: "bollinger_bands",
                parameter: "period",
                value: period,
                requirement: ">= 0",
            });
        }
        let w = period as usize;
        Ok(Self {
            mean_state: RollingMeanState::new(w, w),
            var_state: RollingVarState::new(w, w.max(1), 1),
            num_std,
        })
    }

    /// Commit one completed bar value and advance state.
    pub fn commit(&mut self, val: f64) -> BollingerOutput {
        let middle = self.mean_state.push(val);
        let var = self.var_state.push(val);
        let std = if var < 0.0 { 0.0 } else { var.sqrt() };
        #[expect(
            clippy::suboptimal_flops,
            reason = "pandas evaluates a*b+c unfused; mul_add changes the result bits"
        )]
        {
            let upper = middle + self.num_std * std;
            let lower = middle - self.num_std * std;
            BollingerOutput {
                upper,
                middle,
                lower,
            }
        }
    }

    /// Preview the forming bar value without mutating state.
    pub fn preview(&self, val: f64) -> BollingerOutput {
        let mut clone = self.clone();
        clone.commit(val)
    }

    /// Reset internal state to initial empty conditions.
    pub fn reset(&mut self) {
        self.mean_state.reset();
        self.var_state.reset();
    }
}

/// Streaming state for Relative Strength Index (Wilder / pandas ewm alpha).
#[derive(Debug, Clone)]
pub struct RsiState {
    prev_close: Option<f64>,
    gain_state: EwmMeanState,
    loss_state: EwmMeanState,
}

impl RsiState {
    /// Create a new RSI state with the given period.
    pub fn new(period: i64) -> Result<Self, IndicatorError> {
        if period < 1 {
            return Err(IndicatorError::InvalidParameter {
                function: "rsi",
                parameter: "period",
                value: period,
                requirement: ">= 1",
            });
        }
        let com = com_from_alpha_period(period);
        let min_periods = period as usize;
        Ok(Self {
            prev_close: None,
            gain_state: EwmMeanState::new(com, min_periods),
            loss_state: EwmMeanState::new(com, min_periods),
        })
    }

    /// Commit one completed close value and advance state.
    pub fn commit(&mut self, val: f64) -> f64 {
        let (gain_in, loss_in) = match self.prev_close {
            None => {
                self.prev_close = Some(val);
                (f64::NAN, f64::NAN)
            }
            Some(prev) => {
                let delta = val - prev;
                self.prev_close = Some(val);
                let gain = if delta.is_nan() {
                    f64::NAN
                } else if delta >= 0.0 {
                    delta
                } else {
                    0.0
                };
                let neg_delta = -delta;
                let loss = if neg_delta.is_nan() {
                    f64::NAN
                } else if neg_delta >= 0.0 {
                    neg_delta
                } else {
                    0.0
                };
                (gain, loss)
            }
        };

        let avg_gain = self.gain_state.push(gain_in);
        let avg_loss = self.loss_state.push(loss_in);
        let rs = avg_gain / avg_loss;
        100.0 - (100.0 / (1.0 + rs))
    }

    /// Preview the forming close value without mutating state.
    pub fn preview(&self, val: f64) -> f64 {
        let mut clone = self.clone();
        clone.commit(val)
    }

    /// Reset internal state to initial empty conditions.
    pub fn reset(&mut self) {
        self.prev_close = None;
        self.gain_state.reset();
        self.loss_state.reset();
    }
}

/// Input triple (high, low, close) for ATR.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct AtrInput {
    pub high: f64,
    pub low: f64,
    pub close: f64,
}

impl AtrInput {
    pub const fn new(high: f64, low: f64, close: f64) -> Self {
        Self { high, low, close }
    }
}

impl From<(f64, f64, f64)> for AtrInput {
    fn from(t: (f64, f64, f64)) -> Self {
        Self {
            high: t.0,
            low: t.1,
            close: t.2,
        }
    }
}

/// Streaming state for Average True Range.
#[derive(Debug, Clone)]
pub struct AtrState {
    prev_close: Option<f64>,
    ewm_state: EwmMeanState,
}

impl AtrState {
    /// Create a new ATR state with the given period.
    pub fn new(period: i64) -> Result<Self, IndicatorError> {
        if period < 1 {
            return Err(IndicatorError::InvalidParameter {
                function: "atr",
                parameter: "period",
                value: period,
                requirement: ">= 1",
            });
        }
        let com = com_from_alpha_period(period);
        let min_periods = period as usize;
        Ok(Self {
            prev_close: None,
            ewm_state: EwmMeanState::new(com, min_periods),
        })
    }

    /// Commit one completed bar and advance state.
    pub fn commit(&mut self, input: AtrInput) -> f64 {
        let tr = match self.prev_close {
            None => {
                self.prev_close = Some(input.close);
                input.high - input.low
            }
            Some(prev) => {
                self.prev_close = Some(input.close);
                let tr1 = input.high - input.low;
                let tr2 = (input.high - prev).abs();
                let tr3 = (input.low - prev).abs();
                nan_skipping_max3(tr1, tr2, tr3)
            }
        };

        self.ewm_state.push(tr)
    }

    /// Preview the forming bar without mutating state.
    pub fn preview(&self, input: AtrInput) -> f64 {
        let mut clone = self.clone();
        clone.commit(input)
    }

    /// Reset internal state to initial empty conditions.
    pub fn reset(&mut self) {
        self.prev_close = None;
        self.ewm_state.reset();
    }
}

/// Input bar for Session VWAP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionVwapInput {
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: f64,
    pub session: i64,
}

impl SessionVwapInput {
    pub const fn new(high: f64, low: f64, close: f64, volume: f64, session: i64) -> Self {
        Self {
            high,
            low,
            close,
            volume,
            session,
        }
    }
}

/// Output pair (vwap, std_dev) for Session VWAP.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SessionVwapOutput {
    pub vwap: f64,
    pub std_dev: f64,
}

/// Streaming state for Session VWAP with volume-weighted standard deviation.
#[derive(Debug, Clone, Default)]
pub struct SessionVwapState {
    cum_v: f64,
    sum_v_tp: f64,
    m2: f64,
    prev_vwap: f64,
    current_session: Option<i64>,
}

impl SessionVwapState {
    /// Create a new session VWAP state.
    pub fn new() -> Self {
        Self::default()
    }

    /// Commit one completed bar and advance state.
    pub fn commit(&mut self, input: SessionVwapInput) -> Result<SessionVwapOutput, IndicatorError> {
        if input.volume < 0.0 || !input.volume.is_finite() {
            return Err(IndicatorError::InvalidInput {
                function: "session_vwap",
                parameter: "volume",
                value: input.volume,
                requirement: "finite and >= 0",
            });
        }

        if self.current_session != Some(input.session) {
            self.reset();
            self.current_session = Some(input.session);
        }

        let tp = (input.high + input.low + input.close) / 3.0;

        if input.volume == 0.0 {
            if self.cum_v == 0.0 {
                Ok(SessionVwapOutput {
                    vwap: f64::NAN,
                    std_dev: f64::NAN,
                })
            } else {
                let variance = self.m2 / self.cum_v;
                let std_dev = if variance < 0.0 { 0.0 } else { variance.sqrt() };
                Ok(SessionVwapOutput {
                    vwap: self.prev_vwap,
                    std_dev,
                })
            }
        } else if self.cum_v == 0.0 {
            self.cum_v = input.volume;
            self.sum_v_tp = input.volume * tp;
            self.prev_vwap = self.sum_v_tp / self.cum_v;
            self.m2 = 0.0;
            Ok(SessionVwapOutput {
                vwap: self.prev_vwap,
                std_dev: 0.0,
            })
        } else {
            #[expect(
                clippy::suboptimal_flops,
                reason = "unfused arithmetic preserves exact IEEE summation order across platforms"
            )]
            {
                self.cum_v += input.volume;
                self.sum_v_tp += input.volume * tp;
                let vwap = self.sum_v_tp / self.cum_v;
                let delta = tp - self.prev_vwap;
                let delta2 = tp - vwap;
                self.m2 += input.volume * delta * delta2;
                self.prev_vwap = vwap;
                let variance = self.m2 / self.cum_v;
                let std_dev = if variance < 0.0 { 0.0 } else { variance.sqrt() };
                Ok(SessionVwapOutput { vwap, std_dev })
            }
        }
    }

    /// Preview the forming bar without mutating state.
    pub fn preview(&self, input: SessionVwapInput) -> Result<SessionVwapOutput, IndicatorError> {
        let mut clone = self.clone();
        clone.commit(input)
    }

    /// Reset internal state to initial empty conditions.
    pub fn reset(&mut self) {
        self.cum_v = 0.0;
        self.sum_v_tp = 0.0;
        self.m2 = 0.0;
        self.prev_vwap = 0.0;
        self.current_session = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_f64_eq(a: f64, b: f64) {
        if a.is_nan() && b.is_nan() {
            return;
        }
        assert_eq!(a.to_bits(), b.to_bits(), "expected {b}, got {a}");
    }

    #[test]
    fn sma_invalid_period() {
        assert!(matches!(
            SmaState::new(0),
            Err(IndicatorError::InvalidParameter {
                function: "sma",
                parameter: "period",
                value: 0,
                requirement: ">= 1",
            })
        ));
    }

    #[test]
    fn sma_commit_preview_reset() {
        let mut state = SmaState::new(3).unwrap();
        assert!(state.commit(10.0).is_nan());
        assert!(state.commit(20.0).is_nan());

        // Repeated previews with varying values must not mutate state.
        let prev1 = state.preview(30.0);
        assert_f64_eq(prev1, 20.0);
        let prev2 = state.preview(60.0);
        assert_f64_eq(prev2, 30.0);

        // Commit 30.0 should produce the exact same result as preview(30.0).
        let c1 = state.commit(30.0);
        assert_f64_eq(c1, prev1);

        // Reset clears back to initial.
        state.reset();
        assert!(state.commit(10.0).is_nan());
    }

    #[test]
    fn ema_invalid_period() {
        assert!(matches!(
            EmaState::new(0),
            Err(IndicatorError::InvalidParameter {
                function: "ema",
                parameter: "period",
                value: 0,
                requirement: ">= 1",
            })
        ));
    }

    #[test]
    fn ema_commit_preview_reset() {
        let mut state = EmaState::new(3).unwrap();
        let c0 = state.commit(10.0);
        assert_f64_eq(c0, 10.0);

        let p1 = state.preview(20.0);
        let p2 = state.preview(30.0);
        assert_ne!(p1.to_bits(), p2.to_bits());

        let c1 = state.commit(20.0);
        assert_f64_eq(c1, p1);

        state.reset();
        assert_f64_eq(state.commit(5.0), 5.0);
    }

    #[test]
    fn bollinger_invalid_period() {
        assert!(matches!(
            BollingerState::new(-1, 2.0),
            Err(IndicatorError::InvalidParameter {
                function: "bollinger_bands",
                parameter: "period",
                value: -1,
                requirement: ">= 0",
            })
        ));
    }

    #[test]
    fn bollinger_commit_preview_reset() {
        let mut state = BollingerState::new(2, 2.0).unwrap();
        let c0 = state.commit(10.0);
        assert!(c0.upper.is_nan());
        assert!(c0.middle.is_nan());
        assert!(c0.lower.is_nan());

        let p1 = state.preview(20.0);
        assert!(!p1.upper.is_nan());
        assert_f64_eq(p1.middle, 15.0);

        let c1 = state.commit(20.0);
        assert_f64_eq(c1.middle, p1.middle);
        assert_f64_eq(c1.upper, p1.upper);
        assert_f64_eq(c1.lower, p1.lower);

        state.reset();
        let r0 = state.commit(10.0);
        assert!(r0.middle.is_nan());
    }

    #[test]
    fn rsi_invalid_period() {
        assert!(matches!(
            RsiState::new(0),
            Err(IndicatorError::InvalidParameter {
                function: "rsi",
                parameter: "period",
                value: 0,
                requirement: ">= 1",
            })
        ));
    }

    #[test]
    fn rsi_commit_preview_reset() {
        let mut state = RsiState::new(2).unwrap();
        assert!(state.commit(10.0).is_nan());
        assert!(state.commit(20.0).is_nan());

        let p1 = state.preview(30.0);
        let p2 = state.preview(10.0);
        assert_ne!(p1.to_bits(), p2.to_bits());

        let c1 = state.commit(30.0);
        assert_f64_eq(c1, p1);

        state.reset();
        assert!(state.commit(10.0).is_nan());
    }

    #[test]
    fn atr_invalid_period() {
        assert!(matches!(
            AtrState::new(0),
            Err(IndicatorError::InvalidParameter {
                function: "atr",
                parameter: "period",
                value: 0,
                requirement: ">= 1",
            })
        ));
    }

    #[test]
    fn atr_commit_preview_reset() {
        let mut state = AtrState::new(2).unwrap();
        assert!(state.commit(AtrInput::new(12.0, 8.0, 10.0)).is_nan());

        let p1 = state.preview(AtrInput::new(22.0, 18.0, 20.0));
        let p2 = state.preview(AtrInput::new(32.0, 28.0, 30.0));
        assert_ne!(p1.to_bits(), p2.to_bits());

        let c1 = state.commit(AtrInput::new(22.0, 18.0, 20.0));
        assert_f64_eq(c1, p1);

        state.reset();
        assert!(state.commit(AtrInput::new(12.0, 8.0, 10.0)).is_nan());
    }

    #[test]
    fn session_vwap_hand_computed_fixtures() {
        let mut state = SessionVwapState::new();

        // Bar 0: tp = (12 + 8 + 10)/3 = 10, v = 100, session = 1
        let out0 = state
            .commit(SessionVwapInput::new(12.0, 8.0, 10.0, 100.0, 1))
            .unwrap();
        assert_f64_eq(out0.vwap, 10.0);
        assert_f64_eq(out0.std_dev, 0.0);

        // Preview bar 1
        let prev1 = state
            .preview(SessionVwapInput::new(22.0, 18.0, 20.0, 100.0, 1))
            .unwrap();
        assert_f64_eq(prev1.vwap, 15.0);
        assert_f64_eq(prev1.std_dev, 5.0);

        // Bar 1: tp = (22 + 18 + 20)/3 = 20, v = 100, session = 1
        let out1 = state
            .commit(SessionVwapInput::new(22.0, 18.0, 20.0, 100.0, 1))
            .unwrap();
        assert_f64_eq(out1.vwap, 15.0);
        assert_f64_eq(out1.std_dev, 5.0);
        assert_f64_eq(out1.vwap, prev1.vwap);
        assert_f64_eq(out1.std_dev, prev1.std_dev);

        // Bar 2: tp = (32 + 28 + 30)/3 = 30, v = 200, session = 1
        // Hand calculation: cum_v = 400, sum_v_tp = 9000, vwap = 22.5, var = 68.75
        let out2 = state
            .commit(SessionVwapInput::new(32.0, 28.0, 30.0, 200.0, 1))
            .unwrap();
        assert_f64_eq(out2.vwap, 22.5);
        assert_f64_eq(out2.std_dev, (68.75_f64).sqrt());

        // Bar 3: Session change from 1 to 2!
        let out3 = state
            .commit(SessionVwapInput::new(52.0, 48.0, 50.0, 50.0, 2))
            .unwrap();
        assert_f64_eq(out3.vwap, 50.0);
        assert_f64_eq(out3.std_dev, 0.0);

        // Bar 4: Session 2 continuation
        let out4 = state
            .commit(SessionVwapInput::new(62.0, 58.0, 60.0, 50.0, 2))
            .unwrap();
        assert_f64_eq(out4.vwap, 55.0);
        assert_f64_eq(out4.std_dev, 5.0);

        // Zero-volume opening bar in a new session (session 3)
        let out5 = state
            .commit(SessionVwapInput::new(10.0, 10.0, 10.0, 0.0, 3))
            .unwrap();
        assert!(out5.vwap.is_nan());
        assert!(out5.std_dev.is_nan());

        // Second bar in session 3 has positive volume
        let out6 = state
            .commit(SessionVwapInput::new(20.0, 20.0, 20.0, 10.0, 3))
            .unwrap();
        assert_f64_eq(out6.vwap, 20.0);
        assert_f64_eq(out6.std_dev, 0.0);

        // Third bar in session 3 has zero volume (retains current vwap and std_dev)
        let out7 = state
            .commit(SessionVwapInput::new(30.0, 30.0, 30.0, 0.0, 3))
            .unwrap();
        assert_f64_eq(out7.vwap, 20.0);
        assert_f64_eq(out7.std_dev, 0.0);

        // Session decrease (3 -> 1) resets
        let out8 = state
            .commit(SessionVwapInput::new(100.0, 100.0, 100.0, 10.0, 1))
            .unwrap();
        assert_f64_eq(out8.vwap, 100.0);
        assert_f64_eq(out8.std_dev, 0.0);

        // Invalid volumes
        assert!(matches!(
            state.commit(SessionVwapInput::new(1.0, 1.0, 1.0, -1.0, 1)),
            Err(IndicatorError::InvalidInput { .. })
        ));
        assert!(matches!(
            state.commit(SessionVwapInput::new(1.0, 1.0, 1.0, f64::NAN, 1)),
            Err(IndicatorError::InvalidInput { .. })
        ));
        assert!(matches!(
            state.commit(SessionVwapInput::new(1.0, 1.0, 1.0, f64::INFINITY, 1)),
            Err(IndicatorError::InvalidInput { .. })
        ));
    }
}
