//! Parameter-space vertex (`vp`) token-width round-trip fidelity.
//!
//! Spec §"vp u v w" lets a `vp` line carry 1, 2, or 3 coordinates
//! depending on usage (1D special points, 2D trimming-curve / surface
//! special points, 3D rational trimming-curve control points). The
//! decoder pads every entry to a `[u, v, w]` triple with `0.0`, which
//! is indistinguishable from an explicitly-written `0.0`. A naive
//! encoder that infers the original arity by stripping trailing zeros
//! corrupts any `vp` line whose trailing (or middle) component is a
//! genuine zero:
//!
//!   * `vp 0.5 0.0`         (v = 0 surface special point) → `vp 0.5`
//!   * `vp 0.5 0.5 0.0`     (zero-weight rational trim CP) → `vp 0.5 0.5`
//!
//! The decoder records the source token width into
//! `Scene3D::extras["obj:vp_widths"]` (parallel to `obj:vp`) so the
//! encoder reproduces each line at its original arity. These tests pin
//! that behaviour.

use oxideav_obj::obj;

fn round_trip(src: &str) -> String {
    let scene = obj::parse_obj(src).unwrap();
    let bytes = obj::serialize_obj(&scene, None).unwrap();
    String::from_utf8(bytes).unwrap()
}

/// Extract every emitted `vp …` line from serialised OBJ output.
fn vp_lines(out: &str) -> Vec<String> {
    out.lines()
        .filter(|l| l.starts_with("vp ") || *l == "vp")
        .map(|l| l.to_string())
        .collect()
}

#[test]
fn two_token_vp_with_zero_v_preserves_width() {
    // `vp 0.5 0.0` is a genuine 2-coord parameter point sitting on the
    // v = 0 edge of a surface. The trailing-zero-strip heuristic would
    // wrongly collapse it to `vp 0.5`.
    let out = round_trip("vp 0.5 0.0\nvp 0.25 1.0\n");
    let lines = vp_lines(&out);
    assert_eq!(lines, vec!["vp 0.5 0", "vp 0.25 1"], "got: {lines:?}");
}

#[test]
fn three_token_vp_with_zero_w_preserves_width() {
    // `vp 0.5 0.5 0.0` is a zero-weight rational trimming-curve control
    // point — all three coordinates were written. The heuristic would
    // strip the `0.0` weight down to `vp 0.5 0.5`.
    let out = round_trip("vp 0.5 0.5 0.0\n");
    let lines = vp_lines(&out);
    assert_eq!(lines, vec!["vp 0.5 0.5 0"], "got: {lines:?}");
}

#[test]
fn one_token_vp_stays_one_token() {
    // A 1D curve special point: only `u` is written.
    let out = round_trip("vp 0.75\n");
    let lines = vp_lines(&out);
    assert_eq!(lines, vec!["vp 0.75"], "got: {lines:?}");
}

#[test]
fn mixed_arity_vp_pool_each_keeps_its_own_width() {
    let src = "\
vp 0.1
vp 0.2 0.0
vp 0.3 0.4 0.0
vp 0.5 0.6 0.7
";
    let out = round_trip(src);
    let lines = vp_lines(&out);
    assert_eq!(
        lines,
        vec!["vp 0.1", "vp 0.2 0", "vp 0.3 0.4 0", "vp 0.5 0.6 0.7"],
        "got: {lines:?}"
    );
}

#[test]
fn vp_widths_extras_key_is_populated_and_parallel() {
    let scene = obj::parse_obj("vp 0.1\nvp 0.2 0.3\nvp 0.4 0.5 0.6\n").unwrap();
    let widths = scene
        .extras
        .get("obj:vp_widths")
        .and_then(|v| v.as_array())
        .expect("obj:vp_widths present");
    let vp = scene
        .extras
        .get("obj:vp")
        .and_then(|v| v.as_array())
        .expect("obj:vp present");
    assert_eq!(widths.len(), vp.len(), "widths parallel to vp pool");
    let nums: Vec<u64> = widths.iter().filter_map(|v| v.as_u64()).collect();
    assert_eq!(nums, vec![1, 2, 3]);
}

#[test]
fn double_round_trip_is_stable() {
    // Decode → encode → decode → encode must be a fixed point: the
    // second encode reproduces the first encode byte-for-byte.
    let src = "vp 0.5 0.0\nvp 0.25 0.5 0.0\nvp 0.75\n";
    let first = round_trip(src);
    let second = round_trip(&first);
    assert_eq!(
        first, second,
        "vp emission must be a round-trip fixed point"
    );
}
