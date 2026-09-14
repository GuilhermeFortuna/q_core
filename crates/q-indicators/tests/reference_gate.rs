#![forbid(unsafe_code)]

use q_parity::fixture::load_reference_set;
use q_parity::gate::{parse_pending, run_gate, Binding};
use std::path::Path;

const BINDINGS: &[Binding] = &[]; // Q-022 adds one entry per kernel
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
