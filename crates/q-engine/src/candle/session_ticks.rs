//! Session-backed trade-price source for candle replay.

use super::{IntrabarPrices, IntrabarSource};

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TradePrices {
    pub time_msc: Vec<i64>,
    pub price: Vec<f64>,
}

pub trait SessionLoader {
    fn load(&self, session: i64) -> Result<Option<TradePrices>, String>;
}

pub struct SessionTickBars {
    pub session: Vec<i64>,
    pub start_msc: Vec<i64>,
    pub end_msc: Vec<i64>,
    pub open: Vec<f64>,
    pub high: Vec<f64>,
    pub low: Vec<f64>,
    pub close: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum SessionTickError {
    MissingSession {
        bar: usize,
        session: i64,
    },
    Empty {
        bar: usize,
    },
    Mismatch {
        bar: usize,
        observed: [f64; 4],
        expected: [f64; 4],
    },
    Loader {
        bar: usize,
        reason: String,
    },
    InvalidBar {
        bar: usize,
        reason: String,
    },
}

impl std::fmt::Display for SessionTickError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingSession { bar, session } => {
                write!(f, "bar {bar}: missing session {session}")
            }
            Self::Empty { bar } => write!(f, "bar {bar}: empty tick interval"),
            Self::Mismatch {
                bar,
                observed,
                expected,
            } => write!(
                f,
                "bar {bar}: OHLC mismatch observed {observed:?}, expected {expected:?}"
            ),
            Self::Loader { bar, reason } => write!(f, "bar {bar}: {reason}"),
            Self::InvalidBar { bar, reason } => write!(f, "bar {bar}: {reason}"),
        }
    }
}

pub struct SessionTicks<L> {
    loader: L,
    session: Vec<i64>,
    start_msc: Vec<i64>,
    end_msc: Vec<i64>,
    ohlc: [Vec<f64>; 4],
    cached: std::sync::Mutex<Option<(i64, TradePrices)>>,
}

impl<L: SessionLoader> SessionTicks<L> {
    pub fn new(loader: L, bars: SessionTickBars) -> Result<Self, String> {
        let SessionTickBars {
            session,
            start_msc,
            end_msc,
            open,
            high,
            low,
            close,
        } = bars;
        let n = session.len();
        for (name, len) in [
            ("bar_start_msc", start_msc.len()),
            ("bar_end_msc", end_msc.len()),
            ("open", open.len()),
            ("high", high.len()),
            ("low", low.len()),
            ("close", close.len()),
        ] {
            if len != n {
                return Err(format!("{name}: length {len} does not match expected {n}"));
            }
        }
        Ok(Self {
            loader,
            session,
            start_msc,
            end_msc,
            ohlc: [open, high, low, close],
            cached: std::sync::Mutex::new(None),
        })
    }

    pub fn prices_checked(&self, bar: usize) -> Result<IntrabarPrices, SessionTickError> {
        if bar >= self.session.len() {
            return Err(SessionTickError::InvalidBar {
                bar,
                reason: "bar out of range".into(),
            });
        }
        let session = self.session[bar];
        if self.cached.lock().unwrap().as_ref().map(|entry| entry.0) != Some(session) {
            let loaded = self
                .loader
                .load(session)
                .map_err(|reason| SessionTickError::Loader { bar, reason })?
                .ok_or(SessionTickError::MissingSession { bar, session })?;
            *self.cached.lock().unwrap() = Some((session, loaded));
        }
        let cached = self.cached.lock().unwrap();
        let data = &cached.as_ref().unwrap().1;
        let start = self.start_msc[bar];
        let end = self.end_msc[bar];
        let first = data.time_msc.partition_point(|&t| t < start);
        let last = data.time_msc.partition_point(|&t| t < end);
        if first == last {
            return Err(SessionTickError::Empty { bar });
        }
        let times = &data.time_msc[first..last];
        let prices = &data.price[first..last];
        let observed = [
            prices[0],
            prices.iter().copied().fold(f64::NEG_INFINITY, f64::max),
            prices.iter().copied().fold(f64::INFINITY, f64::min),
            *prices.last().unwrap(),
        ];
        let expected = [
            self.ohlc[0][bar],
            self.ohlc[1][bar],
            self.ohlc[2][bar],
            self.ohlc[3][bar],
        ];
        if observed
            .iter()
            .zip(expected)
            .any(|(a, b)| a.to_bits() != b.to_bits())
        {
            return Err(SessionTickError::Mismatch {
                bar,
                observed,
                expected,
            });
        }
        let time_us = times
            .iter()
            .map(|t| {
                t.checked_mul(1000)
                    .ok_or_else(|| SessionTickError::InvalidBar {
                        bar,
                        reason: "time_msc overflows microseconds".into(),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(IntrabarPrices {
            time_us,
            price: prices.to_vec(),
        })
    }
}

impl<L: SessionLoader> IntrabarSource for SessionTicks<L> {
    fn prices(&self, bar: usize) -> Result<IntrabarPrices, String> {
        self.prices_checked(bar).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    struct Loader(Arc<AtomicUsize>);
    impl SessionLoader for Loader {
        fn load(&self, _: i64) -> Result<Option<TradePrices>, String> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(Some(TradePrices {
                time_msc: vec![10, 11, 12],
                price: vec![1.0, 2.0, 1.5],
            }))
        }
    }
    #[test]
    fn slices_half_open_and_reuses_session() {
        let loads = Arc::new(AtomicUsize::new(0));
        let source = SessionTicks::new(
            Loader(loads.clone()),
            SessionTickBars {
                session: vec![0, 0],
                start_msc: vec![10, 12],
                end_msc: vec![12, 13],
                open: vec![1.0, 1.5],
                high: vec![2.0, 1.5],
                low: vec![1.0, 1.5],
                close: vec![2.0, 1.5],
            },
        )
        .unwrap();
        assert_eq!(
            source.prices_checked(0).unwrap().time_us,
            vec![10_000, 11_000]
        );
        assert_eq!(source.prices_checked(1).unwrap().time_us, vec![12_000]);
        assert_eq!(loads.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn reports_empty_interval_for_bar_without_trades() {
        let source = SessionTicks::new(
            Loader(Arc::new(AtomicUsize::new(0))),
            SessionTickBars {
                session: vec![0],
                start_msc: vec![13],
                end_msc: vec![14],
                open: vec![1.0],
                high: vec![1.0],
                low: vec![1.0],
                close: vec![1.0],
            },
        )
        .unwrap();
        assert_eq!(
            source.prices_checked(0),
            Err(SessionTickError::Empty { bar: 0 })
        );
    }

    #[test]
    fn reports_observed_and_expected_ohlc_on_mismatch() {
        let source = SessionTicks::new(
            Loader(Arc::new(AtomicUsize::new(0))),
            SessionTickBars {
                session: vec![0],
                start_msc: vec![10],
                end_msc: vec![13],
                open: vec![1.0],
                high: vec![3.0],
                low: vec![1.0],
                close: vec![1.5],
            },
        )
        .unwrap();
        assert_eq!(
            source.prices_checked(0),
            Err(SessionTickError::Mismatch {
                bar: 0,
                observed: [1.0, 2.0, 1.0, 1.5],
                expected: [1.0, 3.0, 1.0, 1.5],
            })
        );
    }

    #[test]
    fn replaces_cached_session_only_when_the_ordinal_changes() {
        let loads = Arc::new(AtomicUsize::new(0));
        let source = SessionTicks::new(
            Loader(loads.clone()),
            SessionTickBars {
                session: vec![0, 1, 1],
                start_msc: vec![10, 10, 12],
                end_msc: vec![12, 12, 13],
                open: vec![1.0, 1.0, 1.5],
                high: vec![2.0, 2.0, 1.5],
                low: vec![1.0, 1.0, 1.5],
                close: vec![2.0, 2.0, 1.5],
            },
        )
        .unwrap();
        source.prices_checked(0).unwrap();
        source.prices_checked(1).unwrap();
        source.prices_checked(2).unwrap();
        assert_eq!(loads.load(Ordering::SeqCst), 2);
    }
}
