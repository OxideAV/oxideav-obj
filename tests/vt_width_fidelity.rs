//! Byte-faithful `vt` re-emission per Wavefront OBJ spec §"vt u v w".
//!
//! The spec makes both `v` and `w` optional on a texture vertex, each
//! defaulting to `0`: a 1D texture needs only `u`, a 2D texture needs
//! `u v`, and a 3D texture needs all three. The polygonal pool keeps
//! only the `[u, v]` pair a glTF UV can carry, so without extra
//! bookkeeping a `vt u` (1D) and a `vt u v w` (3D) would both collapse
//! onto the canonical `vt u v` form on re-emit, corrupting the
//! decode → encode round-trip. The decoder records each line's source
//! token width into `Scene3D::extras["obj:texcoord_widths"]` (and the
//! dropped 3rd `w` into `["obj:texcoord_w"]`) so the encoder reproduces
//! the exact arity.

use oxideav_obj::obj;

fn lines_of(bytes: &[u8]) -> Vec<String> {
    String::from_utf8(bytes.to_vec())
        .unwrap()
        .lines()
        .map(str::to_string)
        .collect()
}

fn vt_lines(bytes: &[u8]) -> Vec<String> {
    lines_of(bytes)
        .into_iter()
        .filter(|l| l.starts_with("vt "))
        .collect()
}

/// A plain 2D `vt u v` file leaves the side-channel keys absent and
/// re-emits unchanged — the common case must not regress.
#[test]
fn two_d_vt_unchanged_and_no_extras() {
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0.0 0.0
vt 1.0 0.0
vt 1.0 1.0
f 1/1 2/2 3/3
";
    let scene = obj::parse_obj(src).unwrap();
    assert!(!scene.extras.contains_key("obj:texcoord_widths"));
    assert!(!scene.extras.contains_key("obj:texcoords"));
    let out = obj::serialize_obj(&scene, None).unwrap();
    assert_eq!(vt_lines(&out), vec!["vt 0 0", "vt 1 0", "vt 1 1"],);
}

/// A 1D `vt u` texture coordinate re-emits as a single token, not
/// `vt u 0`.
#[test]
fn one_d_vt_round_trips_single_token() {
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0.25
vt 0.5
vt 0.75
f 1/1 2/2 3/3
";
    let scene = obj::parse_obj(src).unwrap();
    assert_eq!(
        scene
            .extras
            .get("obj:texcoord_widths")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_u64()).collect::<Vec<_>>()),
        Some(vec![1, 1, 1]),
    );
    let out = obj::serialize_obj(&scene, None).unwrap();
    assert_eq!(vt_lines(&out), vec!["vt 0.25", "vt 0.5", "vt 0.75"]);
}

/// A 3D `vt u v w` texture coordinate re-emits all three components,
/// preserving the `w` depth coordinate that the 2D pool drops.
#[test]
fn three_d_vt_round_trips_depth_component() {
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0.0 0.0 0.5
vt 1.0 0.0 1.5
vt 1.0 1.0 2.5
f 1/1 2/2 3/3
";
    let scene = obj::parse_obj(src).unwrap();
    assert!(scene.extras.contains_key("obj:texcoord_w"));
    let out = obj::serialize_obj(&scene, None).unwrap();
    assert_eq!(
        vt_lines(&out),
        vec!["vt 0 0 0.5", "vt 1 0 1.5", "vt 1 1 2.5"],
    );
}

/// A genuine `w == 0` on a 3D line is still re-emitted as three tokens
/// — the width vector, not a trailing-zero heuristic, drives the arity.
/// (The `obj:texcoord_w` key may be absent here since every `w` is the
/// default 0, but `obj:texcoord_widths` carries the 3.)
#[test]
fn three_d_vt_with_zero_depth_keeps_three_tokens() {
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0.5 0.5 0.0
f 1/1 1/1 1/1
";
    let scene = obj::parse_obj(src).unwrap();
    assert_eq!(
        scene
            .extras
            .get("obj:texcoord_widths")
            .and_then(|v| v.as_array())
            .and_then(|a| a.first())
            .and_then(|x| x.as_u64()),
        Some(3),
    );
    let out = obj::serialize_obj(&scene, None).unwrap();
    assert_eq!(vt_lines(&out), vec!["vt 0.5 0.5 0"]);
}

/// Mixed 1D / 2D / 3D `vt` lines in one file each keep their own arity
/// across the round-trip, and the parallel width vector lines up
/// index-for-index with the emitted order.
#[test]
fn mixed_width_vt_lines_each_keep_arity() {
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0.1
vt 0.2 0.3
vt 0.4 0.5 0.6
f 1/1 2/2 3/3
";
    let scene = obj::parse_obj(src).unwrap();
    assert_eq!(
        scene
            .extras
            .get("obj:texcoord_widths")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|x| x.as_u64()).collect::<Vec<_>>()),
        Some(vec![1, 2, 3]),
    );
    let out = obj::serialize_obj(&scene, None).unwrap();
    assert_eq!(
        vt_lines(&out),
        vec!["vt 0.1", "vt 0.2 0.3", "vt 0.4 0.5 0.6"],
    );
}

/// Decode → encode → decode is a fixed point for the mixed-arity file:
/// a second round-trip produces byte-identical `vt` lines.
#[test]
fn mixed_width_vt_is_a_fixed_point() {
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0.1
vt 0.2 0.3
vt 0.4 0.5 0.6
f 1/1 2/2 3/3
";
    let scene1 = obj::parse_obj(src).unwrap();
    let out1 = obj::serialize_obj(&scene1, None).unwrap();
    let scene2 = obj::parse_obj(std::str::from_utf8(&out1).unwrap()).unwrap();
    let out2 = obj::serialize_obj(&scene2, None).unwrap();
    assert_eq!(vt_lines(&out1), vt_lines(&out2));
}

/// Two distinct 3D `vt` lines that share the same `[u, v]` pair but
/// differ in `w` must not collapse onto one another — the depth
/// coordinate keeps them distinct slots.
#[test]
fn shared_uv_distinct_depth_stays_distinct() {
    let src = "\
v 0 0 0
v 1 0 0
v 1 1 0
vt 0.5 0.5 1.0
vt 0.5 0.5 2.0
f 1/1 2/2 3/1
";
    let scene = obj::parse_obj(src).unwrap();
    let out = obj::serialize_obj(&scene, None).unwrap();
    assert_eq!(vt_lines(&out), vec!["vt 0.5 0.5 1", "vt 0.5 0.5 2"],);
}
