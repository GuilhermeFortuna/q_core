#![forbid(unsafe_code)]

use q_parity::fixture::{load_reference_set, ParamValue, Params};
use q_parity::gate::{parse_pending, run_gate, Binding, KernelError, KernelInputs, KernelOutputs};
use std::path::Path;

fn param_i64(params: &Params, name: &str) -> Result<i64, KernelError> {
    match params.get(name) {
        Some(ParamValue::Int(v)) => Ok(*v),
        Some(other) => Err(KernelError {
            message: format!("param '{name}' expected Int, got {other:?}"),
        }),
        None => Err(KernelError {
            message: format!("missing param '{name}'"),
        }),
    }
}

fn param_f64(params: &Params, name: &str) -> Result<f64, KernelError> {
    match params.get(name) {
        Some(ParamValue::Float(v)) => Ok(*v),
        Some(ParamValue::Int(v)) => Ok(*v as f64),
        Some(other) => Err(KernelError {
            message: format!("param '{name}' expected Float or Int, got {other:?}"),
        }),
        None => Err(KernelError {
            message: format!("missing param '{name}'"),
        }),
    }
}

fn rejection(err: q_indicators::IndicatorError) -> KernelError {
    KernelError {
        message: err.to_string(),
    }
}

fn single_out(name: &str, values: Vec<f64>) -> KernelOutputs {
    let mut out = KernelOutputs::new();
    out.insert(name, values);
    out
}

fn bind_close_period(
    inputs: &KernelInputs<'_>,
    params: &Params,
    function_id: &str,
    kernel: fn(&[f64], i64) -> Result<Vec<f64>, q_indicators::IndicatorError>,
) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let period = param_i64(params, "period")?;
    let values = kernel(close, period).map_err(rejection)?;
    Ok(single_out(function_id, values))
}

fn bind_ma_sma(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    bind_close_period(inputs, params, "ma_sma", q_indicators::sma)
}

fn bind_ma_ema(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    bind_close_period(inputs, params, "ma_ema", q_indicators::ema)
}

fn bind_ma_smma(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    bind_close_period(inputs, params, "ma_smma", q_indicators::smma)
}

fn bind_ma_wma(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    bind_close_period(inputs, params, "ma_wma", q_indicators::wma)
}

fn bind_ma_hma(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    bind_close_period(inputs, params, "ma_hma", q_indicators::hma)
}

fn bind_rolling_zscore(
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let window = param_i64(params, "window")?;
    let values = q_indicators::rolling_zscore(close, window).map_err(rejection)?;
    Ok(single_out("rolling_zscore", values))
}

fn bind_rolling_rank(
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let window = param_i64(params, "window")?;
    let values = q_indicators::rolling_rank(close, window).map_err(rejection)?;
    Ok(single_out("rolling_rank", values))
}

fn bind_pct_change(
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let change_bars = param_i64(params, "change_bars")?;
    let values = q_indicators::pct_change(close, change_bars).map_err(rejection)?;
    Ok(single_out("pct_change", values))
}

fn bind_clip(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let low = param_f64(params, "low")?;
    let high = param_f64(params, "high")?;
    let values = q_indicators::clip(close, low, high).map_err(rejection)?;
    Ok(single_out("clip", values))
}

fn bind_realized_vol(
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let window = param_i64(params, "window")?;
    let periods_per_year = param_i64(params, "periods_per_year")?;
    let values = q_indicators::realized_vol(close, window, periods_per_year).map_err(rejection)?;
    Ok(single_out("realized_vol", values))
}

fn bind_yang_zhang(
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<KernelOutputs, KernelError> {
    let open = inputs.column("open")?;
    let high = inputs.column("high")?;
    let low = inputs.column("low")?;
    let close = inputs.column("close")?;
    let window = param_i64(params, "window")?;
    let periods_per_year = param_i64(params, "periods_per_year")?;
    let values = q_indicators::yang_zhang(open, high, low, close, window, periods_per_year)
        .map_err(rejection)?;
    Ok(single_out("yang_zhang", values))
}

fn bind_rsi(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    bind_close_period(inputs, params, "rsi", q_indicators::rsi)
}

fn bind_bollinger_bands(
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let period = param_i64(params, "period")?;
    let num_std = param_f64(params, "num_std")?;
    let bands = q_indicators::bollinger_bands(close, period, num_std).map_err(rejection)?;
    let mut out = KernelOutputs::new();
    out.insert("upper", bands.upper);
    out.insert("middle", bands.middle);
    out.insert("lower", bands.lower);
    Ok(out)
}

fn bind_macd(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    let close = inputs.column("close")?;
    let fast_period = param_i64(params, "fast_period")?;
    let slow_period = param_i64(params, "slow_period")?;
    let signal_period = param_i64(params, "signal_period")?;
    let macd =
        q_indicators::macd(close, fast_period, slow_period, signal_period).map_err(rejection)?;
    let mut out = KernelOutputs::new();
    out.insert("line", macd.line);
    out.insert("signal", macd.signal);
    out.insert("histogram", macd.histogram);
    Ok(out)
}

fn bind_donchian_channels(
    inputs: &KernelInputs<'_>,
    params: &Params,
) -> Result<KernelOutputs, KernelError> {
    let high = inputs.column("high")?;
    let low = inputs.column("low")?;
    let period = param_i64(params, "period")?;
    let ch = q_indicators::donchian_channels(high, low, period).map_err(rejection)?;
    let mut out = KernelOutputs::new();
    out.insert("upper", ch.upper);
    out.insert("lower", ch.lower);
    Ok(out)
}

fn bind_atr(inputs: &KernelInputs<'_>, params: &Params) -> Result<KernelOutputs, KernelError> {
    let high = inputs.column("high")?;
    let low = inputs.column("low")?;
    let close = inputs.column("close")?;
    let period = param_i64(params, "period")?;
    let values = q_indicators::atr(high, low, close, period).map_err(rejection)?;
    Ok(single_out("atr", values))
}

const BINDINGS: &[Binding] = &[
    Binding {
        function_id: "ma_sma",
        kernel: bind_ma_sma,
    },
    Binding {
        function_id: "ma_ema",
        kernel: bind_ma_ema,
    },
    Binding {
        function_id: "ma_smma",
        kernel: bind_ma_smma,
    },
    Binding {
        function_id: "ma_wma",
        kernel: bind_ma_wma,
    },
    Binding {
        function_id: "ma_hma",
        kernel: bind_ma_hma,
    },
    Binding {
        function_id: "rolling_zscore",
        kernel: bind_rolling_zscore,
    },
    Binding {
        function_id: "rolling_rank",
        kernel: bind_rolling_rank,
    },
    Binding {
        function_id: "pct_change",
        kernel: bind_pct_change,
    },
    Binding {
        function_id: "clip",
        kernel: bind_clip,
    },
    Binding {
        function_id: "realized_vol",
        kernel: bind_realized_vol,
    },
    Binding {
        function_id: "yang_zhang",
        kernel: bind_yang_zhang,
    },
    Binding {
        function_id: "rsi",
        kernel: bind_rsi,
    },
    Binding {
        function_id: "bollinger_bands",
        kernel: bind_bollinger_bands,
    },
    Binding {
        function_id: "macd",
        kernel: bind_macd,
    },
    Binding {
        function_id: "donchian_channels",
        kernel: bind_donchian_channels,
    },
    Binding {
        function_id: "atr",
        kernel: bind_atr,
    },
];
const PENDING: &str = include_str!("reference_pending.txt");
const BACKEND_REV: &str = include_str!("../../../BACKEND_REV");

#[test]
fn indicator_reference_gate() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../fixtures/reference");
    let ref_set = load_reference_set(&root, "indicators").expect("load_reference_set failed");
    let pending = parse_pending(PENDING).expect("parse_pending failed");
    let pinned_rev = BACKEND_REV.trim();

    let report = run_gate(&ref_set, BINDINGS, &pending, pinned_rev);
    println!("{}", report.summary());
    report.assert_passed();
}
