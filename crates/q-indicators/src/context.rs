//! Explainable market context kernels and streaming state.
//!
//! Evaluates market context readings (trend, momentum, volatility, and VWAP)
//! from deterministic streaming price studies.

use crate::error::IndicatorError;
use crate::ieee::ieee_eq;
use crate::streaming::{
    AtrInput, AtrState, EmaState, RsiState, SessionVwapInput, SessionVwapOutput, SessionVwapState,
    SmaState,
};

/// Status of an individual market context reading.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextReadingStatus {
    /// Calculation succeeded and reading is active.
    Available,
    /// Sufficient bars or warmup history have not yet accumulated.
    WarmingUp,
    /// Preconditions or inputs necessary for this reading are not satisfied.
    Unavailable,
}

impl ContextReadingStatus {
    /// Returns true if the reading is available.
    #[inline]
    pub const fn is_available(self) -> bool {
        matches!(self, Self::Available)
    }

    /// Returns true if the reading is warming up.
    #[inline]
    pub const fn is_warming_up(self) -> bool {
        matches!(self, Self::WarmingUp)
    }

    /// Returns true if the reading is unavailable.
    #[inline]
    pub const fn is_unavailable(self) -> bool {
        matches!(self, Self::Unavailable)
    }
}

/// Qualitative trend category evaluated from fast and slow EMA alignment and slopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrendCategory {
    /// Fast EMA > Slow EMA and both one-bar slopes > 0.
    Upward,
    /// Fast EMA < Slow EMA and both one-bar slopes < 0.
    Downward,
    /// Fast EMA == Slow EMA and both one-bar slopes == 0.
    Balanced,
    /// All other conditions (e.g. counter-trend slope or mixed slope directions).
    Mixed,
}

/// Numerical evidence underlying the trend reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TrendEvidence {
    pub fast_ema: f64,
    pub slow_ema: f64,
    pub fast_slope: Option<f64>,
    pub slow_slope: Option<f64>,
    pub spread: f64,
    pub normalized_spread: Option<f64>,
}

/// Trend reading combining status, category, reason, and numerical evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct TrendReading {
    pub status: ContextReadingStatus,
    pub category: Option<TrendCategory>,
    pub reason: Option<&'static str>,
    pub evidence: TrendEvidence,
}

/// Qualitative momentum zone evaluated from RSI thresholds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MomentumCategory {
    /// RSI < rsi_lower threshold.
    LowerZone,
    /// rsi_lower <= RSI <= rsi_upper (equality is middle zone).
    MiddleZone,
    /// RSI > rsi_upper threshold.
    UpperZone,
}

/// Alias for [`MomentumCategory`].
pub type MomentumZone = MomentumCategory;

/// Direction of RSI change relative to preceding committed bar.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RsiChangeLabel {
    Rising,
    Falling,
    Unchanged,
}

/// Alias for [`RsiChangeLabel`].
pub type MomentumChange = RsiChangeLabel;

/// Numerical evidence underlying the momentum reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MomentumEvidence {
    pub rsi: f64,
    pub rsi_lower: f64,
    pub rsi_upper: f64,
    pub rsi_change: Option<f64>,
    pub change_label: Option<RsiChangeLabel>,
}

/// Momentum reading combining status, category, reason, and numerical evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct MomentumReading {
    pub status: ContextReadingStatus,
    pub category: Option<MomentumCategory>,
    pub reason: Option<&'static str>,
    pub evidence: MomentumEvidence,
}

/// Qualitative volatility category based on ATR relative to historical baseline SMA.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolatilityCategory {
    /// ATR / baseline < volatility_lower.
    Contracting,
    /// volatility_lower <= ATR / baseline <= volatility_upper (equality is typical).
    Typical,
    /// ATR / baseline > volatility_upper.
    Expanding,
}

/// Numerical evidence underlying the volatility reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VolatilityEvidence {
    pub atr: f64,
    pub baseline_atr: Option<f64>,
    pub ratio: Option<f64>,
    pub lower_threshold: f64,
    pub upper_threshold: f64,
}

/// Volatility reading combining status, category, reason, and numerical evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct VolatilityReading {
    pub status: ContextReadingStatus,
    pub category: Option<VolatilityCategory>,
    pub reason: Option<&'static str>,
    pub evidence: VolatilityEvidence,
}

/// Price location relative to VWAP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VwapPosition {
    Above,
    At,
    Below,
}

/// Extension level relative to VWAP measured in ATR units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VwapExtension {
    Near,
    Extended,
}

/// Categorical reading for VWAP, comprising position and optional extension.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VwapCategory {
    pub position: VwapPosition,
    pub extension: Option<VwapExtension>,
}

/// Numerical evidence underlying the VWAP reading.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VwapEvidence {
    pub vwap: Option<f64>,
    pub distance: Option<f64>,
    pub extension_threshold: f64,
    pub std_dev: Option<f64>,
}

/// VWAP reading combining status, category, reason, and numerical evidence.
#[derive(Debug, Clone, PartialEq)]
pub struct VwapReading {
    pub status: ContextReadingStatus,
    pub category: Option<VwapCategory>,
    pub reason: Option<&'static str>,
    pub evidence: VwapEvidence,
}

/// Complete explainable market context output for a single bar.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextOutput {
    pub trend: TrendReading,
    pub momentum: MomentumReading,
    pub volatility: VolatilityReading,
    pub vwap: VwapReading,
}

/// Configuration parameters for market context kernels.
#[derive(Debug, Clone, PartialEq)]
pub struct ContextConfig {
    pub fast_period: i64,
    pub slow_period: i64,
    pub rsi_period: i64,
    pub rsi_lower: f64,
    pub rsi_upper: f64,
    pub atr_period: i64,
    pub volatility_baseline_period: i64,
    pub volatility_lower: f64,
    pub volatility_upper: f64,
    pub vwap_extension: f64,
}

impl Default for ContextConfig {
    fn default() -> Self {
        Self {
            fast_period: 9,
            slow_period: 21,
            rsi_period: 14,
            rsi_lower: 30.0,
            rsi_upper: 70.0,
            atr_period: 14,
            volatility_baseline_period: 20,
            volatility_lower: 0.8,
            volatility_upper: 1.2,
            vwap_extension: 1.0,
        }
    }
}

impl ContextConfig {
    /// Validates all parameters according to the specification.
    pub fn validate(&self) -> Result<(), IndicatorError> {
        let periods = [
            ("fast_period", self.fast_period),
            ("slow_period", self.slow_period),
            ("rsi_period", self.rsi_period),
            ("atr_period", self.atr_period),
            (
                "volatility_baseline_period",
                self.volatility_baseline_period,
            ),
        ];

        for (name, val) in periods {
            if !(1..=1000).contains(&val) {
                return Err(IndicatorError::InvalidParameter {
                    function: "context",
                    parameter: name,
                    value: val,
                    requirement: "between 1 and 1000",
                });
            }
        }

        if self.fast_period >= self.slow_period {
            return Err(IndicatorError::InvalidParameter {
                function: "context",
                parameter: "fast_period",
                value: self.fast_period,
                requirement: "< slow_period",
            });
        }

        if !self.rsi_lower.is_finite() || self.rsi_lower <= 0.0 || self.rsi_lower >= 100.0 {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "rsi_lower",
                value: self.rsi_lower,
                requirement: "finite and 0 < rsi_lower < 100",
            });
        }

        if !self.rsi_upper.is_finite() || self.rsi_upper <= 0.0 || self.rsi_upper >= 100.0 {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "rsi_upper",
                value: self.rsi_upper,
                requirement: "finite and 0 < rsi_upper < 100",
            });
        }

        if self.rsi_lower >= self.rsi_upper {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "rsi_lower",
                value: self.rsi_lower,
                requirement: "< rsi_upper",
            });
        }

        if !self.volatility_lower.is_finite() || self.volatility_lower <= 0.0 {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "volatility_lower",
                value: self.volatility_lower,
                requirement: "finite and > 0",
            });
        }

        if !self.volatility_upper.is_finite() || self.volatility_upper <= 0.0 {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "volatility_upper",
                value: self.volatility_upper,
                requirement: "finite and > 0",
            });
        }

        if self.volatility_lower >= self.volatility_upper {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "volatility_lower",
                value: self.volatility_lower,
                requirement: "< volatility_upper",
            });
        }

        if !self.vwap_extension.is_finite() || self.vwap_extension <= 0.0 {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "vwap_extension",
                value: self.vwap_extension,
                requirement: "finite and > 0",
            });
        }

        Ok(())
    }
}

/// Bar input data for market context calculation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ContextInput {
    pub open: f64,
    pub high: f64,
    pub low: f64,
    pub close: f64,
    pub volume: Option<f64>,
    pub session_key: i64,
    pub vwap_available: bool,
}

impl ContextInput {
    /// Create a new ContextInput bar.
    pub const fn new(
        open: f64,
        high: f64,
        low: f64,
        close: f64,
        volume: Option<f64>,
        session_key: i64,
        vwap_available: bool,
    ) -> Self {
        Self {
            open,
            high,
            low,
            close,
            volume,
            session_key,
            vwap_available,
        }
    }

    /// Validate the input bar invariants.
    pub fn validate(&self) -> Result<(), IndicatorError> {
        let prices = [
            ("open", self.open),
            ("high", self.high),
            ("low", self.low),
            ("close", self.close),
        ];

        for (name, val) in prices {
            if !val.is_finite() || val <= 0.0 {
                return Err(IndicatorError::InvalidInput {
                    function: "context",
                    parameter: name,
                    value: val,
                    requirement: "finite and > 0",
                });
            }
        }

        if !(self.low <= self.open
            && self.open <= self.high
            && self.low <= self.close
            && self.close <= self.high)
        {
            return Err(IndicatorError::InvalidInput {
                function: "context",
                parameter: "ohlc",
                value: self.high,
                requirement: "low <= open/close <= high",
            });
        }

        if let Some(vol) = self.volume {
            if !vol.is_finite() || vol < 0.0 {
                return Err(IndicatorError::InvalidInput {
                    function: "context",
                    parameter: "volume",
                    value: vol,
                    requirement: "finite and >= 0",
                });
            }
        }

        Ok(())
    }
}

/// Pure classification of trend evidence.
pub fn classify_trend(
    fast: f64,
    slow: f64,
    prev_fast: Option<f64>,
    prev_slow: Option<f64>,
    atr: f64,
) -> TrendReading {
    let spread = fast - slow;
    let normalized_spread = if atr.is_finite() && atr > 0.0 {
        Some(spread / atr)
    } else {
        None
    };

    let (fast_slope, slow_slope) = match (prev_fast, prev_slow) {
        (Some(pf), Some(ps)) => (Some(fast - pf), Some(slow - ps)),
        _ => (None, None),
    };

    let evidence = TrendEvidence {
        fast_ema: fast,
        slow_ema: slow,
        fast_slope,
        slow_slope,
        spread,
        normalized_spread,
    };

    match (fast_slope, slow_slope) {
        (Some(fs), Some(ss)) => {
            let category = if fast > slow && fs > 0.0 && ss > 0.0 {
                TrendCategory::Upward
            } else if fast < slow && fs < 0.0 && ss < 0.0 {
                TrendCategory::Downward
            } else if ieee_eq(fast, slow) && ieee_eq(fs, 0.0) && ieee_eq(ss, 0.0) {
                TrendCategory::Balanced
            } else {
                TrendCategory::Mixed
            };

            TrendReading {
                status: ContextReadingStatus::Available,
                category: Some(category),
                reason: None,
                evidence,
            }
        }
        _ => TrendReading {
            status: ContextReadingStatus::WarmingUp,
            category: None,
            reason: Some("prior bar required for slope"),
            evidence,
        },
    }
}

/// Pure classification of momentum evidence.
pub fn classify_momentum(
    rsi: f64,
    prev_committed_rsi: Option<f64>,
    config: &ContextConfig,
) -> MomentumReading {
    if !rsi.is_finite() {
        return MomentumReading {
            status: ContextReadingStatus::WarmingUp,
            category: None,
            reason: Some("rsi warming up"),
            evidence: MomentumEvidence {
                rsi: f64::NAN,
                rsi_lower: config.rsi_lower,
                rsi_upper: config.rsi_upper,
                rsi_change: None,
                change_label: None,
            },
        };
    }

    let category = if rsi < config.rsi_lower {
        MomentumCategory::LowerZone
    } else if rsi > config.rsi_upper {
        MomentumCategory::UpperZone
    } else {
        MomentumCategory::MiddleZone
    };

    let (rsi_change, change_label) = match prev_committed_rsi {
        Some(prev) if prev.is_finite() => {
            let delta = rsi - prev;
            let label = if delta > 0.0 {
                RsiChangeLabel::Rising
            } else if delta < 0.0 {
                RsiChangeLabel::Falling
            } else {
                RsiChangeLabel::Unchanged
            };
            (Some(delta), Some(label))
        }
        _ => (None, None),
    };

    MomentumReading {
        status: ContextReadingStatus::Available,
        category: Some(category),
        reason: None,
        evidence: MomentumEvidence {
            rsi,
            rsi_lower: config.rsi_lower,
            rsi_upper: config.rsi_upper,
            rsi_change,
            change_label,
        },
    }
}

/// Pure classification of volatility evidence.
pub fn classify_volatility(
    atr: f64,
    baseline_atr: f64,
    config: &ContextConfig,
) -> VolatilityReading {
    let baseline_opt = if baseline_atr.is_finite() {
        Some(baseline_atr)
    } else {
        None
    };

    if !atr.is_finite() || !baseline_atr.is_finite() {
        return VolatilityReading {
            status: ContextReadingStatus::WarmingUp,
            category: None,
            reason: Some("volatility baseline warming up"),
            evidence: VolatilityEvidence {
                atr,
                baseline_atr: baseline_opt,
                ratio: None,
                lower_threshold: config.volatility_lower,
                upper_threshold: config.volatility_upper,
            },
        };
    }

    if baseline_atr <= 0.0 {
        return VolatilityReading {
            status: ContextReadingStatus::Unavailable,
            category: None,
            reason: Some("nonpositive or nonfinite atr baseline"),
            evidence: VolatilityEvidence {
                atr,
                baseline_atr: baseline_opt,
                ratio: None,
                lower_threshold: config.volatility_lower,
                upper_threshold: config.volatility_upper,
            },
        };
    }

    let ratio = atr / baseline_atr;
    let category = if ratio < config.volatility_lower {
        VolatilityCategory::Contracting
    } else if ratio > config.volatility_upper {
        VolatilityCategory::Expanding
    } else {
        VolatilityCategory::Typical
    };

    VolatilityReading {
        status: ContextReadingStatus::Available,
        category: Some(category),
        reason: None,
        evidence: VolatilityEvidence {
            atr,
            baseline_atr: baseline_opt,
            ratio: Some(ratio),
            lower_threshold: config.volatility_lower,
            upper_threshold: config.volatility_upper,
        },
    }
}

/// Pure classification of VWAP evidence.
pub fn classify_vwap(
    close: f64,
    atr: f64,
    vwap_output: Option<SessionVwapOutput>,
    config: &ContextConfig,
    unavailable_reason: Option<&'static str>,
) -> VwapReading {
    let base_evidence = |vwap, dist, sd| VwapEvidence {
        vwap,
        distance: dist,
        extension_threshold: config.vwap_extension,
        std_dev: sd,
    };

    if let Some(reason) = unavailable_reason {
        return VwapReading {
            status: ContextReadingStatus::Unavailable,
            category: None,
            reason: Some(reason),
            evidence: base_evidence(None, None, None),
        };
    }

    let out = match vwap_output {
        Some(o) if o.vwap.is_finite() => o,
        _ => {
            return VwapReading {
                status: ContextReadingStatus::Unavailable,
                category: None,
                reason: Some("zero session volume"),
                evidence: base_evidence(None, None, None),
            };
        }
    };

    let position = if close > out.vwap {
        VwapPosition::Above
    } else if close < out.vwap {
        VwapPosition::Below
    } else {
        VwapPosition::At
    };

    let (distance, extension) = if atr.is_finite() && atr > 0.0 {
        let dist = (close - out.vwap) / atr;
        let ext = if dist.abs() >= config.vwap_extension {
            VwapExtension::Extended
        } else {
            VwapExtension::Near
        };
        (Some(dist), Some(ext))
    } else {
        (None, None)
    };

    VwapReading {
        status: ContextReadingStatus::Available,
        category: Some(VwapCategory {
            position,
            extension,
        }),
        reason: None,
        evidence: base_evidence(Some(out.vwap), distance, Some(out.std_dev)),
    }
}

/// Streaming state machine for explainable market context.
#[derive(Debug, Clone)]
pub struct ContextState {
    config: ContextConfig,
    fast_ema: EmaState,
    slow_ema: EmaState,
    rsi: RsiState,
    atr: AtrState,
    volatility_baseline: SmaState,
    vwap: SessionVwapState,
    last_committed_fast_ema: Option<f64>,
    last_committed_slow_ema: Option<f64>,
    last_committed_rsi: Option<f64>,
}

impl ContextState {
    /// Create a new ContextState with the provided configuration.
    pub fn new(config: ContextConfig) -> Result<Self, IndicatorError> {
        config.validate()?;

        let fast_ema = EmaState::new(config.fast_period)?;
        let slow_ema = EmaState::new(config.slow_period)?;
        let rsi = RsiState::new(config.rsi_period)?;
        let atr = AtrState::new(config.atr_period)?;
        let volatility_baseline = SmaState::new(config.volatility_baseline_period)?;
        let vwap = SessionVwapState::new();

        Ok(Self {
            config,
            fast_ema,
            slow_ema,
            rsi,
            atr,
            volatility_baseline,
            vwap,
            last_committed_fast_ema: None,
            last_committed_slow_ema: None,
            last_committed_rsi: None,
        })
    }

    /// Access the configuration for this state machine.
    #[inline]
    pub fn config(&self) -> &ContextConfig {
        &self.config
    }

    /// Commit one completed bar, mutating internal state and advancing all studies.
    pub fn commit(&mut self, input: ContextInput) -> Result<ContextOutput, IndicatorError> {
        input.validate()?;

        let fast = self.fast_ema.commit(input.close);
        let slow = self.slow_ema.commit(input.close);
        let rsi = self.rsi.commit(input.close);
        let atr = self
            .atr
            .commit(AtrInput::new(input.high, input.low, input.close));
        let baseline_atr = self.volatility_baseline.commit(atr);

        let (vwap_out, vwap_reason) = if !input.vwap_available {
            (None, Some("vwap unavailable"))
        } else if let Some(vol) = input.volume {
            let out = self.vwap.commit(SessionVwapInput::new(
                input.high,
                input.low,
                input.close,
                vol,
                input.session_key,
            ))?;
            (Some(out), None)
        } else {
            (None, Some("missing volume"))
        };

        let trend = classify_trend(
            fast,
            slow,
            self.last_committed_fast_ema,
            self.last_committed_slow_ema,
            atr,
        );
        let momentum = classify_momentum(rsi, self.last_committed_rsi, &self.config);
        let volatility = classify_volatility(atr, baseline_atr, &self.config);
        let vwap = classify_vwap(input.close, atr, vwap_out, &self.config, vwap_reason);

        self.last_committed_fast_ema = Some(fast);
        self.last_committed_slow_ema = Some(slow);
        self.last_committed_rsi = if rsi.is_finite() { Some(rsi) } else { None };

        Ok(ContextOutput {
            trend,
            momentum,
            volatility,
            vwap,
        })
    }

    /// Preview the forming bar without mutating internal state.
    pub fn preview(&self, input: ContextInput) -> Result<ContextOutput, IndicatorError> {
        input.validate()?;

        let fast = self.fast_ema.preview(input.close);
        let slow = self.slow_ema.preview(input.close);
        let rsi = self.rsi.preview(input.close);
        let atr = self
            .atr
            .preview(AtrInput::new(input.high, input.low, input.close));
        let baseline_atr = self.volatility_baseline.preview(atr);

        let (vwap_out, vwap_reason) = if !input.vwap_available {
            (None, Some("vwap unavailable"))
        } else if let Some(vol) = input.volume {
            let out = self.vwap.preview(SessionVwapInput::new(
                input.high,
                input.low,
                input.close,
                vol,
                input.session_key,
            ))?;
            (Some(out), None)
        } else {
            (None, Some("missing volume"))
        };

        let trend = classify_trend(
            fast,
            slow,
            self.last_committed_fast_ema,
            self.last_committed_slow_ema,
            atr,
        );
        let momentum = classify_momentum(rsi, self.last_committed_rsi, &self.config);
        let volatility = classify_volatility(atr, baseline_atr, &self.config);
        let vwap = classify_vwap(input.close, atr, vwap_out, &self.config, vwap_reason);

        Ok(ContextOutput {
            trend,
            momentum,
            volatility,
            vwap,
        })
    }

    /// Reset internal state to initial empty conditions.
    pub fn reset(&mut self) {
        self.fast_ema.reset();
        self.slow_ema.reset();
        self.rsi.reset();
        self.atr.reset();
        self.volatility_baseline.reset();
        self.vwap.reset();
        self.last_committed_fast_ema = None;
        self.last_committed_slow_ema = None;
        self.last_committed_rsi = None;
    }
}

/// Evaluates market context over a slice of input bars.
pub fn batch_context(
    inputs: &[ContextInput],
    config: &ContextConfig,
) -> Result<Vec<ContextOutput>, IndicatorError> {
    config.validate()?;
    let mut state = ContextState::new(config.clone())?;
    let mut outputs = Vec::with_capacity(inputs.len());
    for input in inputs {
        outputs.push(state.commit(*input)?);
    }
    Ok(outputs)
}
