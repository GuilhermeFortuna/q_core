#![forbid(unsafe_code)]

use q_indicators::{
    atr, bollinger_bands, ema, rsi, session_vwap, sma, AtrInput, AtrState, BollingerState,
    EmaState, RsiState, SessionVwapInput, SessionVwapState, SmaState,
};
use q_parity::determinism::check_double_run_with;
use q_parity::fixture::{
    load_reference_set, ColumnData, Expected, ParamValue, Params, ReferenceSet,
};
use std::collections::BTreeMap;
use std::path::Path;

fn assert_bits_eq(actual: f64, expected: f64, ctx: &str) {
    if actual.is_nan() && expected.is_nan() {
        assert_eq!(
            actual.to_bits(),
            expected.to_bits(),
            "{ctx}: NaN bit mismatch: {:#x} != {:#x}",
            actual.to_bits(),
            expected.to_bits()
        );
        return;
    }
    assert_eq!(
        actual.to_bits(),
        expected.to_bits(),
        "{ctx}: got {actual} ({:#x}) expected {expected} ({:#x})",
        actual.to_bits(),
        expected.to_bits()
    );
}

fn param_i64(params: &Params, name: &str) -> i64 {
    match params.get(name) {
        Some(ParamValue::Int(v)) => *v,
        other => panic!("expected int param {name}, got {other:?}"),
    }
}

fn param_f64(params: &Params, name: &str) -> f64 {
    match params.get(name) {
        Some(ParamValue::Float(v)) => *v,
        Some(ParamValue::Int(v)) => *v as f64,
        other => panic!("expected float param {name}, got {other:?}"),
    }
}

fn extract_columns<'a>(
    ref_set: &'a ReferenceSet,
    case_inputs: &BTreeMap<String, q_parity::fixture::ColumnRef>,
) -> BTreeMap<String, &'a [f64]> {
    let mut cols = BTreeMap::new();
    for (name, col_ref) in case_inputs {
        let input_set = ref_set
            .inputs
            .get(&col_ref.input_id)
            .unwrap_or_else(|| panic!("missing input {}", col_ref.input_id));
        let col = input_set
            .columns
            .get(&col_ref.column)
            .unwrap_or_else(|| panic!("missing col {}", col_ref.column));
        if let ColumnData::Float64(vec) = &col.data {
            cols.insert(name.clone(), &vec[..]);
        }
    }
    cols
}

#[test]
fn streaming_parity_sma() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
    let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");
    let fixture = ref_set.functions.get("ma_sma").expect("missing ma_sma");

    for (case_idx, case) in fixture.cases.iter().enumerate() {
        let cols = extract_columns(&ref_set, &case.inputs);
        let close = cols.get("close").expect("missing close");
        let period = param_i64(&case.params, "period");

        match &case.expected {
            Expected::Rejected { .. } => {
                assert!(sma(close, period).is_err());
                assert!(SmaState::new(period).is_err());
            }
            Expected::Outputs(_) => {
                let batch = sma(close, period).expect("batch sma failed");
                let mut state = SmaState::new(period).expect("state creation failed");

                for (i, &val) in close.iter().enumerate() {
                    let _ = state.preview(val + 1.0);
                    let _ = state.preview(val - 1.0);
                    let prev = state.preview(val);

                    let committed = state.commit(val);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("case {case_idx} bar {i} preview/commit"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} batch parity"),
                    );
                }

                state.reset();
                for (i, &val) in close.iter().enumerate() {
                    let committed = state.commit(val);
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} replay"),
                    );
                }
            }
        }
    }

    // Long period test on every reference input
    for (input_name, input_set) in &ref_set.inputs {
        if let Some(col) = input_set.columns.get("close") {
            if let ColumnData::Float64(close) = &col.data {
                let long_period = (close.len() as i64) + 10;
                let batch = sma(close, long_period).expect("batch long_period failed");
                let mut state = SmaState::new(long_period).expect("state long_period failed");
                for (i, &val) in close.iter().enumerate() {
                    let prev = state.preview(val);
                    let committed = state.commit(val);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("{input_name} bar {i} long_period"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("{input_name} bar {i} long_period"),
                    );
                }
            }
        }
    }
}

#[test]
fn streaming_parity_ema() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
    let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");
    let fixture = ref_set.functions.get("ma_ema").expect("missing ma_ema");

    for (case_idx, case) in fixture.cases.iter().enumerate() {
        let cols = extract_columns(&ref_set, &case.inputs);
        let close = cols.get("close").expect("missing close");
        let period = param_i64(&case.params, "period");

        match &case.expected {
            Expected::Rejected { .. } => {
                assert!(ema(close, period).is_err());
                assert!(EmaState::new(period).is_err());
            }
            Expected::Outputs(_) => {
                let batch = ema(close, period).expect("batch ema failed");
                let mut state = EmaState::new(period).expect("state creation failed");

                for (i, &val) in close.iter().enumerate() {
                    let _ = state.preview(val + 10.0);
                    let prev = state.preview(val);
                    let committed = state.commit(val);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("case {case_idx} bar {i} preview/commit"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} batch parity"),
                    );
                }

                state.reset();
                for (i, &val) in close.iter().enumerate() {
                    let committed = state.commit(val);
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} replay"),
                    );
                }
            }
        }
    }

    // Long period test
    for (input_name, input_set) in &ref_set.inputs {
        if let Some(col) = input_set.columns.get("close") {
            if let ColumnData::Float64(close) = &col.data {
                let long_period = (close.len() as i64) + 10;
                let batch = ema(close, long_period).expect("batch long_period failed");
                let mut state = EmaState::new(long_period).expect("state long_period failed");
                for (i, &val) in close.iter().enumerate() {
                    let prev = state.preview(val);
                    let committed = state.commit(val);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("{input_name} bar {i} long_period"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("{input_name} bar {i} long_period"),
                    );
                }
            }
        }
    }
}

#[test]
fn streaming_parity_bollinger_bands() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
    let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");
    let fixture = ref_set
        .functions
        .get("bollinger_bands")
        .expect("missing bollinger_bands");

    for (case_idx, case) in fixture.cases.iter().enumerate() {
        let cols = extract_columns(&ref_set, &case.inputs);
        let close = cols.get("close").expect("missing close");
        let period = param_i64(&case.params, "period");
        let num_std = param_f64(&case.params, "num_std");

        match &case.expected {
            Expected::Rejected { .. } => {
                assert!(bollinger_bands(close, period, num_std).is_err());
                assert!(BollingerState::new(period, num_std).is_err());
            }
            Expected::Outputs(_) => {
                let batch =
                    bollinger_bands(close, period, num_std).expect("batch bollinger failed");
                let mut state =
                    BollingerState::new(period, num_std).expect("state creation failed");

                for (i, &val) in close.iter().enumerate() {
                    let _ = state.preview(val * 1.5);
                    let prev = state.preview(val);
                    let committed = state.commit(val);

                    assert_bits_eq(
                        prev.middle,
                        committed.middle,
                        &format!("case {case_idx} bar {i} mid prev"),
                    );
                    assert_bits_eq(
                        prev.upper,
                        committed.upper,
                        &format!("case {case_idx} bar {i} up prev"),
                    );
                    assert_bits_eq(
                        prev.lower,
                        committed.lower,
                        &format!("case {case_idx} bar {i} low prev"),
                    );

                    assert_bits_eq(
                        committed.middle,
                        batch.middle[i],
                        &format!("case {case_idx} bar {i} mid"),
                    );
                    assert_bits_eq(
                        committed.upper,
                        batch.upper[i],
                        &format!("case {case_idx} bar {i} up"),
                    );
                    assert_bits_eq(
                        committed.lower,
                        batch.lower[i],
                        &format!("case {case_idx} bar {i} low"),
                    );
                }

                state.reset();
                for (i, &val) in close.iter().enumerate() {
                    let committed = state.commit(val);
                    assert_bits_eq(
                        committed.middle,
                        batch.middle[i],
                        &format!("case {case_idx} bar {i} mid replay"),
                    );
                    assert_bits_eq(
                        committed.upper,
                        batch.upper[i],
                        &format!("case {case_idx} bar {i} up replay"),
                    );
                    assert_bits_eq(
                        committed.lower,
                        batch.lower[i],
                        &format!("case {case_idx} bar {i} low replay"),
                    );
                }
            }
        }
    }

    // Long period test
    for (input_name, input_set) in &ref_set.inputs {
        if let Some(col) = input_set.columns.get("close") {
            if let ColumnData::Float64(close) = &col.data {
                let long_period = (close.len() as i64) + 10;
                let batch =
                    bollinger_bands(close, long_period, 2.0).expect("batch long_period failed");
                let mut state =
                    BollingerState::new(long_period, 2.0).expect("state long_period failed");
                for (i, &val) in close.iter().enumerate() {
                    let prev = state.preview(val);
                    let committed = state.commit(val);
                    assert_bits_eq(
                        prev.middle,
                        committed.middle,
                        &format!("{input_name} bar {i} mid long"),
                    );
                    assert_bits_eq(
                        prev.upper,
                        committed.upper,
                        &format!("{input_name} bar {i} up long"),
                    );
                    assert_bits_eq(
                        prev.lower,
                        committed.lower,
                        &format!("{input_name} bar {i} low long"),
                    );
                    assert_bits_eq(
                        committed.middle,
                        batch.middle[i],
                        &format!("{input_name} bar {i} mid long"),
                    );
                    assert_bits_eq(
                        committed.upper,
                        batch.upper[i],
                        &format!("{input_name} bar {i} up long"),
                    );
                    assert_bits_eq(
                        committed.lower,
                        batch.lower[i],
                        &format!("{input_name} bar {i} low long"),
                    );
                }
            }
        }
    }
}

#[test]
fn streaming_parity_rsi() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
    let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");
    let fixture = ref_set.functions.get("rsi").expect("missing rsi");

    for (case_idx, case) in fixture.cases.iter().enumerate() {
        let cols = extract_columns(&ref_set, &case.inputs);
        let close = cols.get("close").expect("missing close");
        let period = param_i64(&case.params, "period");

        match &case.expected {
            Expected::Rejected { .. } => {
                assert!(rsi(close, period).is_err());
                assert!(RsiState::new(period).is_err());
            }
            Expected::Outputs(_) => {
                let batch = rsi(close, period).expect("batch rsi failed");
                let mut state = RsiState::new(period).expect("state creation failed");

                for (i, &val) in close.iter().enumerate() {
                    let _ = state.preview(val + 5.0);
                    let prev = state.preview(val);
                    let committed = state.commit(val);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("case {case_idx} bar {i} preview/commit"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} batch parity"),
                    );
                }

                state.reset();
                for (i, &val) in close.iter().enumerate() {
                    let committed = state.commit(val);
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} replay"),
                    );
                }
            }
        }
    }

    // Long period test
    for (input_name, input_set) in &ref_set.inputs {
        if let Some(col) = input_set.columns.get("close") {
            if let ColumnData::Float64(close) = &col.data {
                let long_period = (close.len() as i64) + 10;
                let batch = rsi(close, long_period).expect("batch long_period failed");
                let mut state = RsiState::new(long_period).expect("state long_period failed");
                for (i, &val) in close.iter().enumerate() {
                    let prev = state.preview(val);
                    let committed = state.commit(val);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("{input_name} bar {i} long_period"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("{input_name} bar {i} long_period"),
                    );
                }
            }
        }
    }
}

#[test]
fn streaming_parity_atr() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
    let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");
    let fixture = ref_set.functions.get("atr").expect("missing atr");

    for (case_idx, case) in fixture.cases.iter().enumerate() {
        let cols = extract_columns(&ref_set, &case.inputs);
        let high = cols.get("high").expect("missing high");
        let low = cols.get("low").expect("missing low");
        let close = cols.get("close").expect("missing close");
        let period = param_i64(&case.params, "period");

        match &case.expected {
            Expected::Rejected { .. } => {
                assert!(atr(high, low, close, period).is_err());
                assert!(AtrState::new(period).is_err());
            }
            Expected::Outputs(_) => {
                let batch = atr(high, low, close, period).expect("batch atr failed");
                let mut state = AtrState::new(period).expect("state creation failed");

                for i in 0..close.len() {
                    let input = AtrInput::new(high[i], low[i], close[i]);
                    let _ = state.preview(AtrInput::new(high[i] + 1.0, low[i] - 1.0, close[i]));
                    let prev = state.preview(input);
                    let committed = state.commit(input);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("case {case_idx} bar {i} preview/commit"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} batch parity"),
                    );
                }

                state.reset();
                for i in 0..close.len() {
                    let input = AtrInput::new(high[i], low[i], close[i]);
                    let committed = state.commit(input);
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("case {case_idx} bar {i} replay"),
                    );
                }
            }
        }
    }

    // Long period test
    for (input_name, input_set) in &ref_set.inputs {
        if let (Some(col_h), Some(col_l), Some(col_c)) = (
            input_set.columns.get("high"),
            input_set.columns.get("low"),
            input_set.columns.get("close"),
        ) {
            if let (
                ColumnData::Float64(high),
                ColumnData::Float64(low),
                ColumnData::Float64(close),
            ) = (&col_h.data, &col_l.data, &col_c.data)
            {
                let long_period = (close.len() as i64) + 10;
                let batch = atr(high, low, close, long_period).expect("batch long_period failed");
                let mut state = AtrState::new(long_period).expect("state long_period failed");
                for i in 0..close.len() {
                    let input = AtrInput::new(high[i], low[i], close[i]);
                    let prev = state.preview(input);
                    let committed = state.commit(input);
                    assert_bits_eq(
                        prev,
                        committed,
                        &format!("{input_name} bar {i} long_period"),
                    );
                    assert_bits_eq(
                        committed,
                        batch[i],
                        &format!("{input_name} bar {i} long_period"),
                    );
                }
            }
        }
    }
}

#[test]
fn session_vwap_parity_and_properties() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
    let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");
    let ohlcv = ref_set
        .inputs
        .get("synthetic_ohlcv_n400")
        .expect("missing synthetic_ohlcv_n400");

    let high = match &ohlcv.columns["high"].data {
        ColumnData::Float64(v) => v,
        _ => panic!("bad data"),
    };
    let low = match &ohlcv.columns["low"].data {
        ColumnData::Float64(v) => v,
        _ => panic!("bad data"),
    };
    let close = match &ohlcv.columns["close"].data {
        ColumnData::Float64(v) => v,
        _ => panic!("bad data"),
    };
    let volume = match &ohlcv.columns["volume"].data {
        ColumnData::Float64(v) => v,
        _ => panic!("bad data"),
    };

    // Synthesize session keys: 4 sessions of 100 bars each
    let session: Vec<i64> = (0..400).map(|i| (i / 100) as i64).collect();

    // 1. Batch vs streaming bitwise parity and preview equivalence
    let batch =
        session_vwap(high, low, close, volume, &session).expect("batch session_vwap failed");
    let mut state = SessionVwapState::new();

    for i in 0..400 {
        let input = SessionVwapInput::new(high[i], low[i], close[i], volume[i], session[i]);
        let _ = state.preview(SessionVwapInput::new(
            high[i] + 10.0,
            low[i] - 10.0,
            close[i],
            volume[i] * 2.0,
            session[i],
        ));
        let prev = state.preview(input).expect("preview failed");
        let committed = state.commit(input).expect("commit failed");

        assert_bits_eq(
            prev.vwap,
            committed.vwap,
            &format!("bar {i} vwap preview/commit"),
        );
        assert_bits_eq(
            prev.std_dev,
            committed.std_dev,
            &format!("bar {i} std_dev preview/commit"),
        );

        assert_bits_eq(
            committed.vwap,
            batch.vwap[i],
            &format!("bar {i} vwap batch parity"),
        );
        assert_bits_eq(
            committed.std_dev,
            batch.std_dev[i],
            &format!("bar {i} std_dev batch parity"),
        );
    }

    // 2. Double-run determinism
    let run1_vwap = session_vwap(high, low, close, volume, &session)
        .unwrap()
        .vwap;
    let run2_vwap = session_vwap(high, low, close, volume, &session)
        .unwrap()
        .vwap;
    check_double_run_with(|| run1_vwap.clone()).expect("double run determinism vwap failed");
    assert_eq!(run1_vwap, run2_vwap);

    let run1_std = session_vwap(high, low, close, volume, &session)
        .unwrap()
        .std_dev;
    let run2_std = session_vwap(high, low, close, volume, &session)
        .unwrap()
        .std_dev;
    check_double_run_with(|| run1_std.clone()).expect("double run determinism std_dev failed");
    assert_eq!(run1_std, run2_std);

    // 3. Prefix causality
    // For every prefix 1..=n, batch session_vwap on that prefix must match the full run on that prefix.
    for len in [1, 5, 50, 99, 100, 101, 150, 200, 350, 400] {
        let prefix_batch = session_vwap(
            &high[..len],
            &low[..len],
            &close[..len],
            &volume[..len],
            &session[..len],
        )
        .expect("prefix batch failed");

        for i in 0..len {
            assert_bits_eq(
                prefix_batch.vwap[i],
                batch.vwap[i],
                &format!("prefix len {len} at {i} vwap causality"),
            );
            assert_bits_eq(
                prefix_batch.std_dev[i],
                batch.std_dev[i],
                &format!("prefix len {len} at {i} std_dev causality"),
            );
        }
    }
}
