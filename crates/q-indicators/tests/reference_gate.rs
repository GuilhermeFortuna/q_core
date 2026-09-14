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

#[allow(dead_code)] // used by clip / bollinger binders in later steps
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
